use std::{sync::Arc, time::Instant};

use jsonrpsee::{
    core::{async_trait, RpcResult},
    proc_macros::rpc,
    types::ErrorObjectOwned,
};
use solana_program_runtime::log_collector::log::info;
use solana_rpc_client_api::config::RpcSendTransactionConfig;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status::{TransactionBinaryEncoding, UiTransactionEncoding};

use crate::{
    application::transaction::{AtlasTransaction, RequestMetadata, SendTransactionBundleResponse},
    errors::{invalid_request, AtlasTxnSenderError},
    storage::transaction_store::TransactionData,
    vendor::solana_rpc::decode_and_deserialize,
};

#[rpc(server)]
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
    fn send_transaction_bundle(
        &self,
        txns: Vec<String>,
        params: RpcSendTransactionConfig,
        request_metadata: Option<RequestMetadata>,
    ) -> RpcResult<SendTransactionBundleResponse>;
}

pub struct AtlasTxnSenderImpl {
    application: Arc<dyn AtlasTransaction + Send + Sync + 'static>,
    max_txn_send_retries: usize,
}

impl AtlasTxnSenderImpl {
    pub fn new(
        application: Arc<dyn AtlasTransaction + Send + Sync + 'static>,
        max_txn_send_retries: usize,
    ) -> Self {
        Self {
            application,
            max_txn_send_retries,
        }
    }
}

impl AtlasTxnSenderImpl {
    fn create_transaction_data(
        &self,
        params: RpcSendTransactionConfig,
        request_metadata: Option<RequestMetadata>,
        binary_encoding: TransactionBinaryEncoding,
        tx_raw: String,
    ) -> RpcResult<TransactionData> {
        let (wire_transaction, versioned_transaction) =
            match decode_and_deserialize::<VersionedTransaction>(tx_raw, binary_encoding) {
                Ok((wire_transaction, versioned_transaction)) => {
                    (wire_transaction, versioned_transaction)
                }
                Err(e) => {
                    return Err(invalid_request(&e.to_string()));
                }
            };

        let transaction = TransactionData {
            wire_transaction,
            versioned_transaction,
            sent_at: Instant::now(),
            retry_count: 0,
            max_retries: std::cmp::min(
                self.max_txn_send_retries,
                params.max_retries.unwrap_or(self.max_txn_send_retries),
            ),
            request_metadata: request_metadata.clone(),
        };

        Ok(transaction)
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
        validate_send_transaction_params(&params)?;
        let encoding = params.encoding.unwrap_or(UiTransactionEncoding::Base58);
        let binary_encoding = encoding.into_binary_encoding().ok_or_else(|| {
            invalid_request(&format!(
                "unsupported encoding: {encoding}. Supported encodings: base58, base64"
            ))
        })?;

        let transaction_data =
            self.create_transaction_data(params, request_metadata.clone(), binary_encoding, txn)?;

        let signature_resp = self
            .application
            .send_transaction(transaction_data, request_metadata)
            .await?;
        Ok(signature_resp)
    }

    fn send_transaction_bundle(
        &self,
        txns: Vec<String>,
        params: RpcSendTransactionConfig,
        request_metadata: Option<RequestMetadata>,
    ) -> RpcResult<SendTransactionBundleResponse> {
        info!("Sending transaction bundle: {:?}", txns);
        validate_send_transaction_params(&params)?;

        let encoding = params.encoding.unwrap_or(UiTransactionEncoding::Base58);
        let binary_encoding = encoding.into_binary_encoding().ok_or_else(|| {
            invalid_request(&format!(
                "unsupported encoding: {encoding}. Supported encodings: base58, base64"
            ))
        })?;

        let mut transactions = Vec::new();

        // Decode and validate all transactions first
        for txn in txns {
            let transaction_data = self.create_transaction_data(
                params,
                request_metadata.clone(),
                binary_encoding,
                txn,
            )?;
            transactions.push(transaction_data);
        }

        let response = self
            .application
            .send_transaction_bundle(transactions, request_metadata)
            .map_err(|e| AtlasTxnSenderError::Custom(e.to_string()))?;

        Ok(response)
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
