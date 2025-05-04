use crate::{transaction_store::TransactionData, txn_sender::TxnSender};
use anyhow::anyhow;
use cadence_macros::statsd_count;
use dashmap::DashMap;
use solana_sdk::clock::Slot;
use solana_sdk::hash::Hash;
use std::sync::Arc;
use tracing::{debug, warn};

pub struct TransactionBundleExecutor {
    txn_sender: Arc<dyn TxnSender>,
    recent_blockhash_cache: Arc<DashMap<Hash, Slot>>,
}

impl TransactionBundleExecutor {
    pub fn new(
        txn_sender: Arc<dyn TxnSender>,
        recent_blockhash_cache: Arc<DashMap<Hash, Slot>>,
    ) -> Self {
        Self {
            txn_sender,
            recent_blockhash_cache,
        }
    }

    async fn wait_for_confirmation(&self, signature: &str, api_key: &str) -> Option<i64> {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(60) {
            if let Some(block_time) = self.txn_sender.get_confirmed_transaction(signature) {
                let block_time_str = block_time.to_string();
                statsd_count!("confirmed_transaction_in_bundle", 1, "api_key" => api_key, "block" => &block_time_str);
                debug!("Transaction confirmed: {:?}", signature);
                self.txn_sender.remove_confirmed_transaction(signature);
                return Some(block_time);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        None
    }

    pub async fn execute_bundle(
        &self,
        transactions: Vec<TransactionData>,
        api_key: &str,
    ) -> anyhow::Result<(Vec<String>, Vec<String>)> {
        let mut signatures = Vec::new();
        let mut errors = Vec::new();

        // Process each transaction individually
        for mut transaction in transactions {
            // Get latest blockhash from cache
            let latest_blockhash = self
                .recent_blockhash_cache
                .iter()
                .max_by_key(|entry| *entry.value())
                .map(|entry| *entry.key())
                .unwrap_or_else(|| {
                    warn!("No valid blockhash found in cache, using default");
                    Hash::default()
                });

            dbg!(latest_blockhash);

            // Update transaction with latest blockhash
            transaction
                .versioned_transaction
                .message
                .set_recent_blockhash(latest_blockhash);
            let signature = transaction.versioned_transaction.signatures[0].to_string();

            debug!("Sending transaction: {}", signature);
            self.txn_sender.send_transaction(transaction.clone());
            debug!("Transaction sent: {:?}", signature);
            statsd_count!("send_transaction_in_bundle", 1, "api_key" => api_key);

            // Wait for confirmation
            match self.wait_for_confirmation(&signature, api_key).await {
                Some(_) => {
                    signatures.push(signature.clone());
                }
                None => {
                    statsd_count!("error_transaction_in_bundle", 1, "api_key" => api_key);
                    warn!("Transaction not confirmed: {:?}", signature);
                    errors.push(format!("Error processing transaction {:?}", signature));
                }
            }
            self.txn_sender
                .remove_confirmed_transaction(signature.as_str());
        }

        if signatures.is_empty() {
            Err(anyhow!("No transactions confirmed"))
        } else {
            Ok((signatures, errors))
        }
    }
}
