use cadence_macros::{statsd_count, statsd_time};
use serde::{Deserialize, Serialize};
use solana_transaction_status::TransactionBinaryEncoding;
use std::sync::Arc;
use tokio::time::Instant;
use tonic::async_trait;
use tracing::info;

use crate::errors::AtlasTxnSenderError;
use crate::infrastructure::txn_sender::TxnSender;
use crate::storage::transaction_store::{TransactionData, TransactionStore};
pub struct AtlasTransactionApp {
    txn_sender: Arc<dyn TxnSender>,
    transaction_store: Arc<dyn TransactionStore>,
}

impl AtlasTransactionApp {
    pub fn new(
        txn_sender: Arc<dyn TxnSender>,
        transaction_store: Arc<dyn TransactionStore>,
    ) -> Self {
        Self {
            txn_sender,
            transaction_store,
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all(serialize = "camelCase", deserialize = "camelCase"))]
pub struct RequestMetadata {
    pub api_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all(serialize = "camelCase", deserialize = "camelCase"))]
pub struct SendTransactionBundleResponse {
    pub signatures: Vec<String>,
    pub errors: Vec<String>,
}

#[async_trait]
pub trait AtlasTransaction: Send + Sync {
    async fn send_transaction(
        &self,
        transaction: TransactionData,
        request_metadata: Option<RequestMetadata>,
    ) -> Result<String, AtlasTxnSenderError>;
    fn send_transaction_bundle(
        &self,
        transactions: Vec<TransactionData>,
        request_metadata: Option<RequestMetadata>,
        binary_encoding: TransactionBinaryEncoding,
    ) -> Result<SendTransactionBundleResponse, AtlasTxnSenderError>;
}

#[async_trait]
impl AtlasTransaction for AtlasTransactionApp {
    async fn send_transaction(
        &self,
        transaction: TransactionData,
        request_metadata: Option<RequestMetadata>,
    ) -> Result<String, AtlasTxnSenderError> {
        let signature = transaction.versioned_transaction.signatures[0].to_string();
        let api_key = request_metadata
            .clone()
            .map(|m| m.api_key)
            .unwrap_or("none".to_string());
        statsd_count!("send_transaction", 1, "api_key" => &api_key);

        if self.transaction_store.has_signature(&signature) {
            statsd_count!("duplicate_transaction", 1, "api_key" => &api_key);
            return Ok(signature);
        }

        self.txn_sender.send_transaction(transaction);
        Ok(signature)
    }

    fn send_transaction_bundle(
        &self,
        transactions: Vec<TransactionData>,
        request_metadata: Option<RequestMetadata>,
        binary_encoding: TransactionBinaryEncoding,
    ) -> Result<SendTransactionBundleResponse, AtlasTxnSenderError> {
        info!(
            "Sending transaction bundle: {:?}",
            transactions
                .iter()
                .map(|t| t.versioned_transaction.signatures[0].to_string())
                .collect::<Vec<String>>()
        );
        let sent_at = Instant::now();
        let api_key = request_metadata
            .clone()
            .map(|m| m.api_key)
            .unwrap_or("none".to_string());
        statsd_count!("send_transaction_bundle", 1, "api_key" => &api_key);
        let (signatures, errors) = self
            .txn_sender
            .send_transaction_bundle(transactions, binary_encoding, &api_key)
            .map_err(|e| AtlasTxnSenderError::Custom(e.to_string()))?;

        statsd_time!(
            "send_transaction_bundle_time",
            sent_at.elapsed(),
            "api_key" => &api_key
        );

        let response = SendTransactionBundleResponse { signatures, errors };

        info!("Transaction bundle sent successfully! {:?}", response);
        Ok(response)
    }
}
