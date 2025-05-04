use cadence_macros::{statsd_count, statsd_time};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::Instant;
use tonic::async_trait;
use tracing::info;

use crate::errors::AtlasTxnSenderError;
use crate::infrastructure::transaction_bundle::TransactionBundleExecutor;
use crate::infrastructure::txn_sender::TxnSender;
use crate::storage::transaction_store::TransactionData;

pub struct AtlasTransactionApp {
    txn_sender: Arc<dyn TxnSender>,
    bundle_executor: Arc<TransactionBundleExecutor>,
}

impl AtlasTransactionApp {
    pub fn new(
        txn_sender: Arc<dyn TxnSender>,
        bundle_executor: Arc<TransactionBundleExecutor>,
    ) -> Self {
        Self {
            txn_sender,
            bundle_executor,
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
pub trait AtlasTransaction {
    async fn send_transaction(
        &self,
        txn_data: TransactionData,
        request_metadata: Option<RequestMetadata>,
    ) -> Result<String, AtlasTxnSenderError>;
    async fn send_transaction_bundle(
        &self,
        txns: Vec<TransactionData>,
        request_metadata: Option<RequestMetadata>,
    ) -> Result<SendTransactionBundleResponse, AtlasTxnSenderError>;
}

#[async_trait]
impl AtlasTransaction for AtlasTransactionApp {
    async fn send_transaction(
        &self,
        txn_data: TransactionData,
        request_metadata: Option<RequestMetadata>,
    ) -> Result<String, AtlasTxnSenderError> {
        info!(
            "Sending transaction: {:?}",
            txn_data.versioned_transaction.signatures[0]
        );
        let start = Instant::now();
        let api_key = request_metadata
            .clone()
            .map(|m| m.api_key)
            .unwrap_or("none".to_string());
        statsd_count!("send_transaction", 1, "api_key" => &api_key);
        self.txn_sender.send_transaction(txn_data.clone());

        let signature = txn_data.versioned_transaction.signatures[0].to_string();

        statsd_time!(
            "send_transaction_time",
            start.elapsed(),
            "api_key" => &api_key
        );
        Ok(signature)
    }

    async fn send_transaction_bundle(
        &self,
        txns: Vec<TransactionData>,
        request_metadata: Option<RequestMetadata>,
    ) -> Result<SendTransactionBundleResponse, AtlasTxnSenderError> {
        info!(
            "Sending transaction bundle: {:?}",
            txns.iter()
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
            .bundle_executor
            .execute_bundle(txns, &api_key)
            .await
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
