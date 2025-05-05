// code copied out of solana repo because of version conflicts

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use bincode::Options;
use jsonrpsee::core::RpcResult;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    bs58, commitment_config::CommitmentConfig, packet::PACKET_DATA_SIZE,
    transaction::VersionedTransaction,
};
use solana_transaction_status::TransactionBinaryEncoding;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

use crate::errors::invalid_request;

const MAX_BASE58_SIZE: usize = 1683; // Golden, bump if PACKET_DATA_SIZE changes
const MAX_BASE64_SIZE: usize = 1644; // Golden, bump if PACKET_DATA_SIZE changes
const DEFAULT_RETRY_COUNT: u32 = 3;
const DEFAULT_RETRY_DELAY_MS: u64 = 1000;
const DEFAULT_CONFIRMATION_TIMEOUT_MS: u64 = 30000;

#[derive(Debug)]
pub struct SendTransactionConfig {
    pub commitment: CommitmentConfig,
    pub retry_count: u32,
    pub retry_delay_ms: u64,
    pub confirmation_timeout_ms: u64,
    pub skip_preflight: bool,
}

impl Default for SendTransactionConfig {
    fn default() -> Self {
        Self {
            commitment: CommitmentConfig::confirmed(),
            retry_count: DEFAULT_RETRY_COUNT,
            retry_delay_ms: DEFAULT_RETRY_DELAY_MS,
            confirmation_timeout_ms: DEFAULT_CONFIRMATION_TIMEOUT_MS,
            skip_preflight: false,
        }
    }
}

pub fn send_and_encode_transaction(
    rpc_client: &RpcClient,
    transaction: &VersionedTransaction,
    encoding: TransactionBinaryEncoding,
    config: Option<SendTransactionConfig>,
) -> RpcResult<String> {
    let config = config.unwrap_or_default();

    // First serialize the transaction
    let wire_output = bincode::options()
        .with_limit(PACKET_DATA_SIZE as u64)
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .serialize(transaction)
        .map_err(|err| invalid_request(&format!("failed to serialize: {}", &err.to_string())))?;

    if wire_output.len() > PACKET_DATA_SIZE {
        return Err(invalid_request("serialized too large"));
    }

    // Send the transaction with retries
    let mut last_error = None;
    for attempt in 0..=config.retry_count {
        match rpc_client.send_transaction_with_config(
            transaction,
            solana_client::rpc_config::RpcSendTransactionConfig {
                skip_preflight: config.skip_preflight,
                preflight_commitment: Some(config.commitment.commitment),
                max_retries: Some(0), // We handle retries ourselves
                min_context_slot: None,
                encoding: None, // Use default encoding
            },
        ) {
            Ok(signature) => {
                info!(
                    "Transaction sent successfully with signature: {}",
                    signature
                );

                // Wait for confirmation
                let confirmation_start = Instant::now();
                while confirmation_start.elapsed()
                    < Duration::from_millis(config.confirmation_timeout_ms)
                {
                    match rpc_client.get_signature_status(&signature) {
                        Ok(Some(status)) => {
                            match status {
                                Ok(_) => {
                                    debug!("Transaction confirmed: {}", signature);
                                    // Encode the wire output based on requested encoding
                                    let encoded = match encoding {
                                        TransactionBinaryEncoding::Base58 => {
                                            bs58::encode(&wire_output).into_string()
                                        }
                                        TransactionBinaryEncoding::Base64 => {
                                            BASE64_STANDARD.encode(&wire_output)
                                        }
                                    };
                                    return Ok(encoded);
                                }
                                Err(err) => {
                                    return Err(invalid_request(&format!(
                                        "Transaction failed: {:?}",
                                        err
                                    )));
                                }
                            }
                        }
                        Ok(None) => {
                            // Transaction not yet confirmed, continue waiting
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Err(e) => {
                            error!("Error checking transaction status: {}", e);
                            break;
                        }
                    }
                }
                return Err(invalid_request("Transaction confirmation timeout"));
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < config.retry_count {
                    debug!(
                        "Failed to send transaction (attempt {}/{}), retrying in {}ms...",
                        attempt + 1,
                        config.retry_count,
                        config.retry_delay_ms
                    );
                    std::thread::sleep(Duration::from_millis(config.retry_delay_ms));
                }
            }
        }
    }

    Err(invalid_request(&format!(
        "Failed to send transaction after {} attempts: {:?}",
        config.retry_count, last_error
    )))
}

pub fn encode_and_serialize<T>(
    transaction: &T,
    encoding: TransactionBinaryEncoding,
) -> RpcResult<String>
where
    T: serde::Serialize,
{
    let wire_output = bincode::options()
        .with_limit(PACKET_DATA_SIZE as u64)
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .serialize(transaction)
        .map_err(|err| invalid_request(&format!("failed to serialize: {}", &err.to_string())))?;

    if wire_output.len() > PACKET_DATA_SIZE {
        return Err(invalid_request("serialized too large"));
    }

    match encoding {
        TransactionBinaryEncoding::Base58 => Ok(bs58::encode(&wire_output).into_string()),
        TransactionBinaryEncoding::Base64 => Ok(BASE64_STANDARD.encode(&wire_output)),
    }
}

pub fn decode_and_deserialize<T>(
    encoded: String,
    encoding: TransactionBinaryEncoding,
) -> RpcResult<(Vec<u8>, T)>
where
    T: serde::de::DeserializeOwned,
{
    let wire_output = match encoding {
        TransactionBinaryEncoding::Base58 => {
            if encoded.len() > MAX_BASE58_SIZE {
                return Err(invalid_request("base58 encoded too large"));
            }
            bs58::decode(encoded)
                .into_vec()
                .map_err(|e| invalid_request(format!("invalid base58 encoding: {e:?}").as_str()))?
        }
        TransactionBinaryEncoding::Base64 => {
            if encoded.len() > MAX_BASE64_SIZE {
                return Err(invalid_request("base64 encoded too large"));
            }
            BASE64_STANDARD
                .decode(encoded)
                .map_err(|e| invalid_request(&format!("invalid base64 encoding: {e:?}")))?
        }
    };
    if wire_output.len() > PACKET_DATA_SIZE {
        return Err(invalid_request("decoded too large"));
    }
    bincode::options()
        .with_limit(PACKET_DATA_SIZE as u64)
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .deserialize_from(&wire_output[..])
        .map_err(|err| invalid_request(&format!("failed to deserialize: {}", &err.to_string())))
        .map(|output| (wire_output, output))
}
