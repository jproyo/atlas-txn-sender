use std::{sync::Arc, time::Instant};

use cadence_macros::{statsd_count, statsd_time};
use jsonrpsee::{
    core::{async_trait, RpcResult},
    proc_macros::rpc,
    types::ErrorObjectOwned,
};
use serde::Deserialize;
use solana_rpc_client_api::config::RpcSendTransactionConfig;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status::UiTransactionEncoding;
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::{
    errors::invalid_request,
    solana_rpc::SolanaRpc,
    transaction_store::{TransactionData, TransactionStore},
    txn_sender::TxnSender,
    vendor::solana_rpc::decode_and_deserialize,
};

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all(serialize = "camelCase", deserialize = "camelCase"))]
pub struct RequestMetadata {
    pub api_key: String,
}

#[rpc(server, namespace = "atlas")]
pub trait AtlasTxnSender {
    #[method(name = "health")]
    async fn health(&self) -> String;
    #[method(name = "sendTransaction")]
    async fn send_transaction(
        &self,
        txn: String,
        params: RpcSendTransactionConfig,
        request_metadata: Option<RequestMetadata>,
    ) -> RpcResult<String>;
    #[method(name = "sendTransactionBundle")]
    async fn send_transaction_bundle(
        &self,
        txns: Vec<String>,
        params: RpcSendTransactionConfig,
        request_metadata: Option<RequestMetadata>,
    ) -> RpcResult<Vec<String>>;
}

pub struct AtlasTxnSenderImpl {
    txn_sender: Arc<dyn TxnSender>,
    transaction_store: Arc<dyn TransactionStore>,
    solana_rpc: Arc<dyn SolanaRpc>,
    max_txn_send_retries: usize,
    bundle_lock: Arc<Mutex<()>>,
}

impl AtlasTxnSenderImpl {
    pub fn new(
        txn_sender: Arc<dyn TxnSender>,
        transaction_store: Arc<dyn TransactionStore>,
        solana_rpc: Arc<dyn SolanaRpc>,
        max_txn_send_retries: usize,
    ) -> Self {
        Self {
            txn_sender,
            max_txn_send_retries,
            transaction_store,
            solana_rpc,
            bundle_lock: Arc::new(Mutex::new(())),
        }
    }
}

#[async_trait]
impl AtlasTxnSenderServer for AtlasTxnSenderImpl {
    async fn health(&self) -> String {
        "ok".to_string()
    }

    async fn send_transaction(
        &self,
        txn: String,
        params: RpcSendTransactionConfig,
        request_metadata: Option<RequestMetadata>,
    ) -> RpcResult<String> {
        let sent_at = Instant::now();
        let api_key = request_metadata
            .clone()
            .map(|m| m.api_key)
            .unwrap_or("none".to_string());
        statsd_count!("send_transaction", 1, "api_key" => &api_key);
        validate_send_transaction_params(&params)?;
        let start = Instant::now();
        let encoding = params.encoding.unwrap_or(UiTransactionEncoding::Base58);
        let binary_encoding = encoding.into_binary_encoding().ok_or_else(|| {
            invalid_request(&format!(
                "unsupported encoding: {encoding}. Supported encodings: base58, base64"
            ))
        })?;
        let (wire_transaction, versioned_transaction) =
            match decode_and_deserialize::<VersionedTransaction>(txn, binary_encoding) {
                Ok((wire_transaction, versioned_transaction)) => {
                    (wire_transaction, versioned_transaction)
                }
                Err(e) => {
                    return Err(invalid_request(&e.to_string()));
                }
            };
        let signature = versioned_transaction.signatures[0].to_string();
        if self.transaction_store.has_signature(&signature) {
            statsd_count!("duplicate_transaction", 1, "api_key" => &api_key);
            return Ok(signature);
        }
        let transaction = TransactionData {
            wire_transaction,
            versioned_transaction,
            sent_at,
            retry_count: 0,
            max_retries: std::cmp::min(
                self.max_txn_send_retries,
                params.max_retries.unwrap_or(self.max_txn_send_retries),
            ),
            request_metadata,
        };
        self.txn_sender.send_transaction(transaction);
        statsd_time!(
            "send_transaction_time",
            start.elapsed(),
            "api_key" => &api_key
        );
        Ok(signature)
    }

    async fn send_transaction_bundle(
        &self,
        txns: Vec<String>,
        params: RpcSendTransactionConfig,
        request_metadata: Option<RequestMetadata>,
    ) -> RpcResult<Vec<String>> {
        let sent_at = Instant::now();
        let api_key = request_metadata
            .clone()
            .map(|m| m.api_key)
            .unwrap_or("none".to_string());
        statsd_count!("send_transaction_bundle", 1, "api_key" => &api_key);
        validate_send_transaction_params(&params)?;

        let encoding = params.encoding.unwrap_or(UiTransactionEncoding::Base58);
        let binary_encoding = encoding.into_binary_encoding().ok_or_else(|| {
            invalid_request(&format!(
                "unsupported encoding: {encoding}. Supported encodings: base58, base64"
            ))
        })?;

        let mut transactions = Vec::new();
        let mut signatures = Vec::new();

        // Decode and validate all transactions first
        for txn in txns {
            let (wire_transaction, versioned_transaction) =
                match decode_and_deserialize::<VersionedTransaction>(txn, binary_encoding) {
                    Ok((wire_transaction, versioned_transaction)) => {
                        (wire_transaction, versioned_transaction)
                    }
                    Err(e) => {
                        return Err(invalid_request(&e.to_string()));
                    }
                };

            let signature = versioned_transaction.signatures[0].to_string();
            if self.transaction_store.has_signature(&signature) {
                statsd_count!("duplicate_transaction", 1, "api_key" => &api_key);
                signatures.push(signature);
                continue;
            }

            let transaction = TransactionData {
                wire_transaction,
                versioned_transaction,
                sent_at,
                retry_count: 0,
                max_retries: std::cmp::min(
                    self.max_txn_send_retries,
                    params.max_retries.unwrap_or(self.max_txn_send_retries),
                ),
                request_metadata: request_metadata.clone(),
            };

            transactions.push(transaction);
            signatures.push(signature);
        }

        // Execute transactions serially with a lock to ensure no other bundles interfere
        let _lock = self.bundle_lock.lock().await;

        for (i, transaction) in transactions.into_iter().enumerate() {
            let signature = signatures[i].clone();
            self.txn_sender.send_transaction(transaction);

            // Wait for confirmation
            match self.solana_rpc.confirm_transaction(signature.clone()).await {
                Some(_) => {
                    info!("Transaction {} confirmed successfully", signature);
                }
                None => {
                    error!("Transaction {} failed or timed out", signature);
                    break; // Stop processing remaining transactions
                }
            }
        }

        statsd_time!(
            "send_transaction_bundle_time",
            sent_at.elapsed(),
            "api_key" => &api_key
        );

        Ok(signatures)
    }
}

fn validate_send_transaction_params(
    params: &RpcSendTransactionConfig,
) -> Result<(), ErrorObjectOwned> {
    if !params.skip_preflight {
        return Err(invalid_request("running preflight check is not supported"));
    }
    Ok(())
}
