use std::{sync::Arc, time::Instant};

use cadence_macros::statsd_time;
use dashmap::DashMap;
use solana_sdk::transaction::VersionedTransaction;
use tracing::debug;
use tracing::error;

use crate::rpc_server::RequestMetadata;

#[derive(Clone)]
pub struct TransactionData {
    pub wire_transaction: Vec<u8>,
    pub versioned_transaction: VersionedTransaction,
    pub sent_at: Instant,
    pub retry_count: usize,
    pub max_retries: usize,
    // might not be the best spot but is easy to add for what we need out of metrics now
    pub request_metadata: Option<RequestMetadata>,
}

pub trait TransactionStore: Send + Sync {
    fn add_transaction(&self, transaction: TransactionData);
    fn get_signatures(&self) -> Vec<String>;
    fn remove_transaction(&self, signature: String) -> Option<TransactionData>;
    fn get_transactions(&self) -> Arc<DashMap<String, TransactionData>>;
    fn has_signature(&self, signature: &str) -> bool;
    fn add_confirmed_transaction(&self, signature: String, block_time: i64);
    fn get_confirmed_transaction(&self, signature: &str) -> Option<i64>;
    fn remove_confirmed_transaction(&self, signature: &str) -> Option<i64>;
}

pub struct TransactionStoreImpl {
    transactions: Arc<DashMap<String, TransactionData>>,
    confirmed_transactions: Arc<DashMap<String, i64>>,
}

impl TransactionStoreImpl {
    pub fn new() -> Self {
        Self {
            transactions: Arc::new(DashMap::new()),
            confirmed_transactions: Arc::new(DashMap::new()),
        }
    }
}

impl Default for TransactionStoreImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionStore for TransactionStoreImpl {
    fn has_signature(&self, signature: &str) -> bool {
        self.transactions.contains_key(signature)
    }
    fn add_transaction(&self, transaction: TransactionData) {
        let start = Instant::now();
        if let Some(signature) = get_signature(&transaction) {
            if self.transactions.contains_key(&signature) {
                return;
            }
            self.transactions.insert(signature.to_string(), transaction);
        } else {
            error!("Transaction has no signatures");
        }
        statsd_time!("add_signature_time", start.elapsed());
    }
    fn get_signatures(&self) -> Vec<String> {
        let start = Instant::now();
        let signatures = self
            .transactions
            .iter()
            .map(|t| get_signature(&t).unwrap())
            .collect();
        statsd_time!("get_signatures_time", start.elapsed());
        signatures
    }
    fn remove_transaction(&self, signature: String) -> Option<TransactionData> {
        let start = Instant::now();
        let transaction = self.transactions.remove(&signature);
        statsd_time!("remove_signature_time", start.elapsed());
        transaction.map(|t| t.1)
    }
    fn get_transactions(&self) -> Arc<DashMap<String, TransactionData>> {
        self.transactions.clone()
    }
    fn add_confirmed_transaction(&self, signature: String, block_time: i64) {
        debug!(
            "Adding confirmed transaction: {} at block time {}",
            signature, block_time
        );
        self.confirmed_transactions.insert(signature, block_time);
    }
    fn get_confirmed_transaction(&self, signature: &str) -> Option<i64> {
        self.confirmed_transactions
            .get(signature)
            .map(|v| *v.value())
    }
    fn remove_confirmed_transaction(&self, signature: &str) -> Option<i64> {
        self.confirmed_transactions
            .remove(signature)
            .map(|(_, v)| v)
    }
}

pub fn get_signature(transaction: &TransactionData) -> Option<String> {
    transaction
        .versioned_transaction
        .signatures
        .first()
        .map(|s| s.to_string())
}
