use crate::{Contract, ContractError};
use std::marker::PhantomData;

use ethers_core::{
    abi::{Abi, Token, Tokenize},
    types::{
        transaction::eip2718::TypedTransaction, BlockNumber, Bytes, TransactionReceipt,
        TransactionRequest,
    },
};
use ethers_providers::Middleware;

#[cfg(not(feature = "legacy"))]
use ethers_core::types::Eip1559TransactionRequest;

use std::sync::Arc;

/// Helper which manages the deployment transaction of a smart contract.
///
/// This is just a wrapper type for [Deployer] with an additional type to convert the [Contract]
/// that the deployer returns when sending the transaction.
#[derive(Debug, Clone)]
#[must_use = "Deployer does nothing unless you `send` it"]
pub struct ContractDeployer<M, C> {
    /// the actual deployer, exposed for overriding the defaults
    pub deployer: Deployer<M>,
    /// marker for the `Contract` type to create afterwards
    ///
    /// this type will be used to construct it via `From::from(Contract)`
    _contract: PhantomData<C>,
}

impl<M: Middleware, C: From<Contract<M>>> ContractDeployer<M, C> {
    /// Create a new instance of this [ContractDeployer]
    pub fn new(deployer: Deployer<M>) -> Self {
        Self { deployer, _contract: Default::default() }
    }

    /// Sets the number of confirmations to wait for the contract deployment transaction
    pub fn confirmations<T: Into<usize>>(mut self, confirmations: T) -> Self {
        self.deployer.confs = confirmations.into();
        self
    }

    pub fn block<T: Into<BlockNumber>>(mut self, block: T) -> Self {
        self.deployer.block = block.into();
        self
    }

    /// Uses a Legacy transaction instead of an EIP-1559 one to do the deployment
    pub fn legacy(mut self) -> Self {
        self.deployer = self.deployer.legacy();
        self
    }

    /// Dry runs the deployment of the contract
    ///
    /// Note: this function _does not_ send a transaction from your account
    pub async fn call(&self) -> Result<(), ContractError<M>> {
        self.deployer.call().await
    }

    /// Broadcasts the contract deployment transaction and after waiting for it to
    /// be sufficiently confirmed (default: 1), it returns a new instance of the contract type at
    /// the deployed contract's address.
    pub async fn send(self) -> Result<C, ContractError<M>> {
        let contract = self.deployer.send().await?;
        Ok(C::from(contract))
    }

    /// Broadcasts the contract deployment transaction and after waiting for it to
    /// be sufficiently confirmed (default: 1), it returns a new instance of the contract type at
    /// the deployed contract's address and the corresponding
    /// [`TransactionReceipt`](ethers_core::types::TransactionReceipt).
    pub async fn send_with_receipt(self) -> Result<(C, TransactionReceipt), ContractError<M>> {
        let (contract, receipt) = self.deployer.send_with_receipt().await?;
        Ok((C::from(contract), receipt))
    }

    /// Returns a reference to the deployer's ABI
    pub fn abi(&self) -> &Abi {
        self.deployer.abi()
    }

    /// Returns a reference to the deployer's client
    pub fn client(&self) -> &M {
        self.deployer.client()
    }
}

/// Helper which manages the deployment transaction of a smart contract
#[derive(Debug, Clone)]
#[must_use = "Deployer does nothing unless you `send` it"]
pub struct Deployer<M> {
    /// The deployer's transaction, exposed for overriding the defaults
    pub tx: TypedTransaction,
    abi: Abi,
    client: Arc<M>,
    confs: usize,
    block: BlockNumber,
}

impl<M: Middleware> Deployer<M> {
    /// Sets the number of confirmations to wait for the contract deployment transaction
    pub fn confirmations<T: Into<usize>>(mut self, confirmations: T) -> Self {
        self.confs = confirmations.into();
        self
    }

    pub fn block<T: Into<BlockNumber>>(mut self, block: T) -> Self {
        self.block = block.into();
        self
    }

    /// Uses a Legacy transaction instead of an EIP-1559 one to do the deployment
    pub fn legacy(mut self) -> Self {
        self.tx = match self.tx {
            TypedTransaction::Eip1559(inner) => {
                let tx: TransactionRequest = inner.into();
                TypedTransaction::Legacy(tx)
            }
            other => other,
        };
        self
    }

    /// Dry runs the deployment of the contract
    ///
    /// Note: this function _does not_ send a transaction from your account
    pub async fn call(&self) -> Result<(), ContractError<M>> {
        self.client
            .call(&self.tx, Some(self.block.into()))
            .await
            .map_err(ContractError::MiddlewareError)?;

        // TODO: It would be nice to handle reverts in a structured way.
        Ok(())
    }

    /// Broadcasts the contract deployment transaction and after waiting for it to
    /// be sufficiently confirmed (default: 1), it returns a [`Contract`](crate::Contract)
    /// struct at the deployed contract's address.
    pub async fn send(self) -> Result<Contract<M>, ContractError<M>> {
        let (contract, _) = self.send_with_receipt().await?;
        Ok(contract)
    }

    /// Broadcasts the contract deployment transaction and after waiting for it to
    /// be sufficiently confirmed (default: 1), it returns a tuple with
    /// the [`Contract`](crate::Contract) struct at the deployed contract's address
    /// and the corresponding [`TransactionReceipt`](ethers_core::types::TransactionReceipt).
    pub async fn send_with_receipt(
        self,
    ) -> Result<(Contract<M>, TransactionReceipt), ContractError<M>> {
        let pending_tx = self
            .client
            .send_transaction(self.tx, Some(self.block.into()))
            .await
            .map_err(ContractError::MiddlewareError)?;

        // TODO: Should this be calculated "optimistically" by address/nonce?
        let receipt = pending_tx
            .confirmations(self.confs)
            .await
            .map_err(|_| ContractError::ContractNotDeployed)?
            .ok_or(ContractError::ContractNotDeployed)?;
        let address = receipt.contract_address.ok_or(ContractError::ContractNotDeployed)?;

        let contract = Contract::new(address, self.abi.clone(), self.client);
        Ok((contract, receipt))
    }

    /// Returns a reference to the deployer's ABI
    pub fn abi(&self) -> &Abi {
        &self.abi
    }

    /// Returns a reference to the deployer's client
    pub fn client(&self) -> &M {
        &self.client
    }
}

/// To deploy a contract to the Ethereum network, a `ContractFactory` can be
/// created which manages the Contract bytecode and Application Binary Interface
/// (ABI), usually generated from the Solidity compiler.
///
/// Once the factory's deployment transaction is mined with sufficient confirmations,
/// the [`Contract`](crate::Contract) object is returned.
///
/// # Example
///
/// ```no_run
/// use ethers_solc::Solc;
/// use ethers_contract::ContractFactory;
/// use ethers_providers::{Provider, Http};
/// use ethers_signers::Wallet;
/// use std::convert::TryFrom;
///
/// # async fn foo() -> Result<(), Box<dyn std::error::Error>> {
/// // first we'll compile the contract (you can alternatively compile it yourself
/// // and pass the ABI/Bytecode
/// let compiled = Solc::default().compile_source("./tests/contract.sol").unwrap();
/// let contract = compiled
///     .get("./tests/contract.sol", "SimpleStorage")
///     .expect("could not find contract");
///
/// // connect to the network
/// let client = Provider::<Http>::try_from("http://localhost:8545").unwrap();
/// let client = std::sync::Arc::new(client);
///
/// // create a factory which will be used to deploy instances of the contract
/// let factory = ContractFactory::new(contract.abi.unwrap().clone(), contract.bytecode().unwrap().clone(), client);
///
/// // The deployer created by the `deploy` call exposes a builder which gets consumed
/// // by the async `send` call
/// let contract = factory
///     .deploy("initial value".to_string())?
///     .confirmations(0usize)
///     .send()
///     .await?;
/// println!("{}", contract.address());
/// # Ok(())
/// # }
#[derive(Debug, Clone)]
pub struct ContractFactory<M> {
    client: Arc<M>,
    abi: Abi,
    bytecode: Bytes,
}

impl<M: Middleware> ContractFactory<M> {
    /// Creates a factory for deployment of the Contract with bytecode, and the
    /// constructor defined in the abi. The client will be used to send any deployment
    /// transaction.
    pub fn new(abi: Abi, bytecode: Bytes, client: Arc<M>) -> Self {
        Self { client, abi, bytecode }
    }

    pub fn deploy_tokens(self, params: Vec<Token>) -> Result<Deployer<M>, ContractError<M>> {
        // Encode the constructor args & concatenate with the bytecode if necessary
        let data: Bytes = match (self.abi.constructor(), params.is_empty()) {
            (None, false) => return Err(ContractError::ConstructorError),
            (None, true) => self.bytecode.clone(),
            (Some(constructor), _) => {
                constructor.encode_input(self.bytecode.to_vec(), &params)?.into()
            }
        };

        // create the tx object. Since we're deploying a contract, `to` is `None`
        // We default to EIP-1559 transactions, but the sender can convert it back
        // to a legacy one
        #[cfg(feature = "legacy")]
        let tx = TransactionRequest { to: None, data: Some(data), ..Default::default() };
        #[cfg(not(feature = "legacy"))]
        let tx = Eip1559TransactionRequest { to: None, data: Some(data), ..Default::default() };
        let tx = tx.into();

        Ok(Deployer {
            client: Arc::clone(&self.client), // cheap clone behind the arc
            abi: self.abi,
            tx,
            confs: 1,
            block: BlockNumber::Latest,
        })
    }

    /// Constructs the deployment transaction based on the provided constructor
    /// arguments and returns a `Deployer` instance. You must call `send()` in order
    /// to actually deploy the contract.
    ///
    /// Notes:
    /// 1. If there are no constructor arguments, you should pass `()` as the argument.
    /// 1. The default poll duration is 7 seconds.
    /// 1. The default number of confirmations is 1 block.
    pub fn deploy<T: Tokenize>(self, constructor_args: T) -> Result<Deployer<M>, ContractError<M>> {
        self.deploy_tokens(constructor_args.into_tokens())
    }
}
