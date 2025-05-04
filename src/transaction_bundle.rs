use anyhow::anyhow;
use cadence_macros::statsd_count;
use solana_sdk::clock::Slot;
use solana_sdk::hash::Hash;
use std::sync::Arc;

use crate::{
    solana_rpc::SolanaRpc,
    transaction_store::{TransactionData, TransactionStore},
    txn_sender::TxnSender,
};

pub struct TransactionBundleExecutor {
    txn_sender: Arc<dyn TxnSender>,
    transaction_store: Arc<dyn TransactionStore>,
    solana_rpc: Arc<dyn SolanaRpc>,
    recent_blockhash_cache: Arc<dashmap::DashMap<Hash, Slot>>,
}

impl TransactionBundleExecutor {
    pub fn new(
        txn_sender: Arc<dyn TxnSender>,
        transaction_store: Arc<dyn TransactionStore>,
        solana_rpc: Arc<dyn SolanaRpc>,
        recent_blockhash_cache: Arc<dashmap::DashMap<Hash, Slot>>,
    ) -> Self {
        Self {
            txn_sender,
            transaction_store,
            solana_rpc,
            recent_blockhash_cache,
        }
    }

    pub async fn execute_bundle(
        &self,
        transactions: Vec<TransactionData>,
        api_key: &str,
    ) -> anyhow::Result<Vec<String>> {
        let mut signatures = Vec::new();
        let mut valid_transactions = Vec::new();

        // First pass: validate blockhashes and collect valid transactions
        for transaction in transactions {
            let signature = transaction.versioned_transaction.signatures[0].to_string();
            let blockhash = transaction.versioned_transaction.message.recent_blockhash();

            // Check if blockhash is still valid or if it's the default blockhash and cache is empty
            if let Some(_slot) = self.recent_blockhash_cache.get(blockhash) {
                // Blockhash is still valid, proceed with transaction
                valid_transactions.push(transaction);
                signatures.push(signature);
            } else if *blockhash == Hash::default() && self.recent_blockhash_cache.is_empty() {
                // Accept default blockhash if cache is empty
                valid_transactions.push(transaction);
                signatures.push(signature);
            } else {
                // Blockhash is no longer valid, skip this transaction
                statsd_count!("invalid_blockhash", 1, "api_key" => api_key);
                continue;
            }
        }

        dbg!(valid_transactions.len());

        // Second pass: execute valid transactions serially
        for transaction in valid_transactions {
            let signature = transaction.versioned_transaction.signatures[0].to_string();

            // Send transaction
            self.txn_sender.send_transaction(transaction.clone());
            statsd_count!("send_transaction_in_bundle", 1, "api_key" => api_key);

            // Wait for confirmation
            match self.solana_rpc.confirm_transaction(signature.clone()).await {
                Some(_) => {
                    statsd_count!("confirmed_transaction_in_bundle", 1, "api_key" => api_key);
                    self.transaction_store.add_transaction(transaction);
                }
                None => {
                    statsd_count!("error_transaction_in_bundle", 1, "api_key" => api_key);
                    return Err(anyhow!("Error processing transaction {}", signature));
                }
            }
        }

        if signatures.is_empty() {
            return Err(anyhow!("No transactions were confirmed"));
        }

        Ok(signatures)
    }
}
