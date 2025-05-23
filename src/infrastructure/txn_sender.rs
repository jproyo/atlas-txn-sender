use cadence_macros::{statsd_count, statsd_gauge, statsd_time};
use dashmap::DashMap;
use solana_client::{
    connection_cache::ConnectionCache, nonblocking::tpu_connection::TpuConnection,
    rpc_client::RpcClient,
};
use solana_program_runtime::{
    compute_budget::ComputeBudget,
    compute_budget_processor::{
        process_compute_budget_instructions, DEFAULT_INSTRUCTION_COMPUTE_UNIT_LIMIT,
        MAX_COMPUTE_UNIT_LIMIT,
    },
    prioritization_fee::{PrioritizationFeeDetails, PrioritizationFeeType},
};
use solana_sdk::{clock::Slot, hash::Hash, transaction::VersionedTransaction};
use solana_transaction_status::TransactionBinaryEncoding;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    runtime::{Builder, Runtime},
    time::{sleep, timeout},
};
use tonic::async_trait;
use tracing::{debug, error, warn};

use super::{leader_tracker::LeaderTracker, solana_rpc::SolanaRpc};
use crate::{
    storage::transaction_store::{get_signature, TransactionData, TransactionStore},
    vendor::solana_rpc::send_and_encode_transaction,
};
use solana_sdk::borsh1::try_from_slice_unchecked;
use solana_sdk::compute_budget::ComputeBudgetInstruction;

const RETRY_COUNT_BINS: [i32; 6] = [0, 1, 2, 5, 10, 25];
const MAX_RETRIES_BINS: [i32; 5] = [0, 1, 5, 10, 30];
const MAX_TIMEOUT_SEND_DATA: Duration = Duration::from_millis(500);
const MAX_TIMEOUT_SEND_DATA_BATCH: Duration = Duration::from_millis(500);
const SEND_TXN_RETRIES: usize = 10;

pub trait TxnSender: Send + Sync {
    fn send_transaction(&self, txn: TransactionData);
    fn send_transaction_bundle(
        &self,
        transactions: Vec<TransactionData>,
        binary_encoding: TransactionBinaryEncoding,
        api_key: &str,
    ) -> anyhow::Result<(Vec<String>, Vec<String>)>;
}

pub struct TxnSenderConfig {
    txn_sender_threads: usize,
    txn_send_retry_interval_seconds: usize,
    max_retry_queue_size: Option<usize>,
    recent_blockhash_cache: Arc<DashMap<Hash, Slot>>,
}

impl TxnSenderConfig {
    pub fn new(
        txn_sender_threads: usize,
        txn_send_retry_interval_seconds: usize,
        max_retry_queue_size: Option<usize>,
        recent_blockhash_cache: Arc<DashMap<Hash, Slot>>,
    ) -> Self {
        Self {
            txn_sender_threads,
            txn_send_retry_interval_seconds,
            max_retry_queue_size,
            recent_blockhash_cache,
        }
    }
}

pub struct TxnSenderImpl {
    leader_tracker: Arc<dyn LeaderTracker>,
    transaction_store: Arc<dyn TransactionStore>,
    connection_cache: Arc<ConnectionCache>,
    solana_rpc: Arc<dyn SolanaRpc>,
    txn_sender_runtime: Arc<Runtime>,
    config: TxnSenderConfig,
    rpc_client: Arc<RpcClient>,
}

impl TxnSenderImpl {
    pub fn new(
        leader_tracker: Arc<dyn LeaderTracker>,
        transaction_store: Arc<dyn TransactionStore>,
        connection_cache: Arc<ConnectionCache>,
        solana_rpc: Arc<dyn SolanaRpc>,
        rpc_client: Arc<RpcClient>,
        config: TxnSenderConfig,
    ) -> Self {
        let txn_sender_runtime = Builder::new_multi_thread()
            .worker_threads(config.txn_sender_threads)
            .enable_all()
            .build()
            .unwrap();
        let txn_sender = Self {
            leader_tracker,
            transaction_store,
            connection_cache,
            solana_rpc,
            rpc_client,
            txn_sender_runtime: Arc::new(txn_sender_runtime),
            config,
        };
        txn_sender.retry_transactions();
        txn_sender
    }

    fn retry_transactions(&self) {
        let leader_tracker = self.leader_tracker.clone();
        let transaction_store = self.transaction_store.clone();
        let connection_cache = self.connection_cache.clone();
        let txn_sender_runtime = self.txn_sender_runtime.clone();
        let txn_send_retry_interval_seconds = self.config.txn_send_retry_interval_seconds;
        let max_retry_queue_size = self.config.max_retry_queue_size;
        tokio::spawn(async move {
            loop {
                let mut transactions_reached_max_retries = vec![];
                let transaction_map = transaction_store.get_transactions();
                let queue_length = transaction_map.len();
                statsd_gauge!("transaction_retry_queue_length", queue_length as u64);

                // Shed transactions by retry_count, if necessary.
                if let Some(max_size) = max_retry_queue_size {
                    if queue_length > max_size {
                        warn!(
                            "Transaction retry queue length is over the limit of {}: {}. Load shedding transactions with highest retry count.", 
                            max_size,
                            queue_length
                        );
                        let mut transactions: Vec<(String, TransactionData)> = transaction_map
                            .iter()
                            .map(|x| (x.key().to_owned(), x.value().to_owned()))
                            .collect();
                        transactions.sort_by(|(_, a), (_, b)| a.retry_count.cmp(&b.retry_count));
                        let transactions_to_remove = transactions[(max_size + 1)..].to_vec();
                        for (signature, _) in transactions_to_remove {
                            transaction_store.remove_transaction(signature.clone());
                            transaction_map.remove(&signature);
                        }
                        let records_dropped = queue_length - max_size;
                        statsd_gauge!("transactions_retry_queue_dropped", records_dropped as u64);
                    }
                }

                let mut wire_transactions = vec![];
                for mut transaction_data in transaction_map.iter_mut() {
                    wire_transactions.push(transaction_data.wire_transaction.clone());
                    if transaction_data.retry_count >= transaction_data.max_retries {
                        transactions_reached_max_retries
                            .push(get_signature(&transaction_data).unwrap());
                    } else {
                        transaction_data.retry_count += 1;
                    }
                }
                for wire_transaction in wire_transactions.iter() {
                    let mut leader_num = 0;
                    for leader in leader_tracker.get_leaders() {
                        if leader.tpu_quic.is_none() {
                            error!("leader {:?} has no tpu_quic", leader);
                            continue;
                        }
                        let connection_cache = connection_cache.clone();
                        let sent_at = Instant::now();
                        let leader = Arc::new(leader.clone());
                        let wire_transaction = wire_transaction.clone();
                        txn_sender_runtime.spawn(async move {
                        // retry unless its a timeout
                        for i in 0..SEND_TXN_RETRIES {
                            let conn = connection_cache.get_nonblocking_connection(&leader.tpu_quic.unwrap());
                            if let Ok(result) = timeout(MAX_TIMEOUT_SEND_DATA_BATCH, conn.send_data(&wire_transaction)).await {
                                if let Err(e) = result {
                                    if i == SEND_TXN_RETRIES-1 {
                                        error!(
                                            retry = "true",
                                            "Failed to send transaction batch to {:?}: {}",
                                            leader, e
                                        );
                                        statsd_count!("transaction_send_error", 1, "retry" => "true", "last_attempt" => "true");
                                    } else {
                                        statsd_count!("transaction_send_error", 1, "retry" => "true", "last_attempt" => "false");
                                    }
                                } else {
                                    let leader_num_str = leader_num.to_string();
                                    statsd_time!(
                                        "transaction_received_by_leader",
                                        sent_at.elapsed(), "leader_num" => &leader_num_str, "api_key" => "not_applicable", "retry" => "true");
                                    return;
                                }
                            } else {
                                // Note: This is far too frequent to log. It will fill the disks on the host and cost too much on DD.
                                statsd_count!("transaction_send_timeout", 1);
                            }
                        }
                    });
                        leader_num += 1;
                    }
                }
                // remove transactions that reached max retries
                for signature in transactions_reached_max_retries {
                    let _ = transaction_store.remove_transaction(signature);
                    statsd_count!("transactions_reached_max_retries", 1);
                }
                sleep(Duration::from_secs(txn_send_retry_interval_seconds as u64)).await;
            }
        });
    }

    fn track_transaction(&self, transaction_data: &TransactionData) {
        let sent_at = transaction_data.sent_at;
        let signature = get_signature(transaction_data);
        debug!("Tracking transaction: {:?}", signature.clone());
        if signature.is_none() {
            return;
        }
        let signature = signature.unwrap();
        self.transaction_store
            .add_transaction(transaction_data.clone());
        let PriorityDetails {
            fee,
            cu_limit,
            priority,
        } = compute_priority_details(&transaction_data.versioned_transaction);
        let priority_fees_enabled = (fee > 0).to_string();
        let solana_rpc = self.solana_rpc.clone();
        let transaction_store = self.transaction_store.clone();
        let api_key = transaction_data
            .request_metadata
            .clone()
            .map(|m| m.api_key.clone())
            .unwrap_or("none".to_string());
        self.txn_sender_runtime.spawn(async move {
            debug!("Confirming transaction: {}", signature);
            let confirmed_at = solana_rpc.confirm_transaction(signature.clone()).await;
            let transcation_data = transaction_store.remove_transaction(signature.clone());
            let mut retries = None;
            let mut max_retries = None;
            if let Some(transaction_data) = transcation_data {
                retries = Some(transaction_data.retry_count as i32);
                max_retries = Some(transaction_data.max_retries as i32);
            }

            let retries_tag = bin_counter_to_tag(retries, RETRY_COUNT_BINS.as_ref());
            let max_retries_tag: String = bin_counter_to_tag(max_retries, MAX_RETRIES_BINS.as_ref());

            // Collect metrics
            // We separate the retry metrics to reduce the cardinality with API key and price.
            let landed = if confirmed_at.is_some() {
                statsd_count!("transactions_landed", 1, "priority_fees_enabled" => &priority_fees_enabled, "retries" => &retries_tag, "max_retries_tag" => &max_retries_tag);
                statsd_count!("transactions_landed_by_key", 1, "api_key" => &api_key);
                statsd_time!("transaction_land_time", sent_at.elapsed(), "api_key" => &api_key, "priority_fees_enabled" => &priority_fees_enabled);
                "true"
            } else {
                debug!("Transaction not landed: {}", signature);
                statsd_count!("transactions_not_landed", 1, "priority_fees_enabled" => &priority_fees_enabled, "retries" => &retries_tag, "max_retries_tag" => &max_retries_tag);
                statsd_count!("transactions_not_landed_by_key", 1, "api_key" => &api_key);
                statsd_count!("transactions_not_landed_retries", 1, "priority_fees_enabled" => &priority_fees_enabled, "retries" => &retries_tag, "max_retries_tag" => &max_retries_tag);
                "false"
            };
            statsd_time!("transaction_priority", priority, "landed" => &landed);
            statsd_time!("transaction_priority_fee", fee, "landed" => &landed);
            statsd_time!("transaction_compute_limit", cu_limit as u64, "landed" => &landed);
        });
    }
}

pub struct PriorityDetails {
    pub fee: u64,
    pub cu_limit: u32,
    pub priority: u64,
}

pub fn compute_priority_details(transaction: &VersionedTransaction) -> PriorityDetails {
    let mut cu_limit = DEFAULT_INSTRUCTION_COMPUTE_UNIT_LIMIT;
    let instructions = transaction.message.instructions().iter().map(|ix| {
        match try_from_slice_unchecked(&ix.data) {
            Ok(ComputeBudgetInstruction::SetComputeUnitLimit(compute_unit_limit)) => {
                cu_limit = compute_unit_limit.min(MAX_COMPUTE_UNIT_LIMIT);
            }
            Ok(_) => {}
            Err(_) => {}
        }
        (
            transaction
                .message
                .static_account_keys()
                .get(usize::from(ix.program_id_index))
                .expect("program id index is sanitized"),
            ix,
        )
    });
    let compute_budget = ComputeBudget::try_from_instructions(instructions);
    let instructions = transaction.message.instructions().iter().map(|ix| {
        match try_from_slice_unchecked(&ix.data) {
            Ok(ComputeBudgetInstruction::SetComputeUnitLimit(compute_unit_limit)) => {
                cu_limit = compute_unit_limit.min(MAX_COMPUTE_UNIT_LIMIT);
            }
            Ok(_) => {}
            Err(_) => {}
        }
        (
            transaction
                .message
                .static_account_keys()
                .get(usize::from(ix.program_id_index))
                .expect("program id index is sanitized"),
            ix,
        )
    });
    let compute_budget_limits = process_compute_budget_instructions(instructions);
    match (compute_budget, compute_budget_limits) {
        (Ok(compute_budget), Ok(compute_budget_limits)) => {
            let priority_fee_type =
                PrioritizationFeeType::ComputeUnitPrice(compute_budget_limits.compute_unit_price);
            let fee =
                PrioritizationFeeDetails::new(priority_fee_type, compute_budget.compute_unit_limit);
            PriorityDetails {
                fee: fee.get_fee(),
                priority: fee.get_priority(),
                cu_limit,
            }
        }
        _ => PriorityDetails {
            fee: 0,
            priority: 0,
            cu_limit,
        },
    }
}

#[async_trait]
impl TxnSender for TxnSenderImpl {
    fn send_transaction(&self, transaction_data: TransactionData) {
        self.track_transaction(&transaction_data);
        let api_key = transaction_data
            .request_metadata
            .map(|m| m.api_key)
            .unwrap_or("none".to_string());
        let mut leader_num = 0;
        for leader in self.leader_tracker.get_leaders() {
            if leader.tpu_quic.is_none() {
                error!("leader {:?} has no tpu_quic", leader);
                continue;
            }
            let connection_cache = self.connection_cache.clone();
            let wire_transaction = transaction_data.wire_transaction.clone();
            let api_key = api_key.clone();
            self.txn_sender_runtime.spawn(async move {
                for i in 0..SEND_TXN_RETRIES {
                    let conn =
                        connection_cache.get_nonblocking_connection(&leader.tpu_quic.unwrap());
                    if let Ok(result) = timeout(MAX_TIMEOUT_SEND_DATA, conn.send_data(&wire_transaction)).await {
                            if let Err(e) = result {
                                if i == SEND_TXN_RETRIES-1 {
                                    error!(
                                        retry = "false",
                                        "Failed to send transaction to {:?}: {}",
                                        leader, e
                                    );
                                    statsd_count!("transaction_send_error", 1, "retry" => "false", "last_attempt" => "true");
                                } else {
                                    statsd_count!("transaction_send_error", 1, "retry" => "false", "last_attempt" => "false");
                                }
                        } else {
                            let leader_num_str = leader_num.to_string();
                            debug!("Transaction received by leader: {:?}", leader_num_str);
                            statsd_time!(
                                "transaction_received_by_leader",
                                transaction_data.sent_at.elapsed(), "leader_num" => &leader_num_str, "api_key" => &api_key, "retry" => "false");
                            return;
                        }
                    } else {
                        // Note: This is far too frequent to log. It will fill the disks on the host and cost too much on DD.
                        statsd_count!("transaction_send_timeout", 1);
                    }
                }
            });
            leader_num += 1;
        }
    }

    fn send_transaction_bundle(
        &self,
        transactions: Vec<TransactionData>,
        binary_encoding: TransactionBinaryEncoding,
        api_key: &str,
    ) -> anyhow::Result<(Vec<String>, Vec<String>)> {
        let mut signatures = Vec::new();
        let mut errors = Vec::new();

        // Process each transaction individually
        for transaction in transactions {
            // Get transaction's blockhash
            let tx_blockhash = transaction.versioned_transaction.message.recent_blockhash();

            // Check if blockhash is still valid
            let is_valid = self
                .config
                .recent_blockhash_cache
                .iter()
                .any(|entry| entry.key() == tx_blockhash);

            if !is_valid {
                warn!("Transaction has expired blockhash: {}", tx_blockhash);
                errors.push(format!(
                    "Transaction has expired blockhash: {}",
                    tx_blockhash
                ));
                continue;
            }

            let signature = transaction.versioned_transaction.signatures[0].to_string();

            debug!("Sending transaction: {}", signature);
            match send_and_encode_transaction(
                &self.rpc_client,
                &transaction.versioned_transaction,
                binary_encoding,
                None,
            ) {
                Ok(signature) => {
                    debug!("Transaction sent and confirmed: {:?}", signature);
                    statsd_count!("send_transaction_in_bundle", 1, "api_key" => api_key);
                    signatures.push(signature.clone());
                }
                Err(e) => {
                    warn!("Error sending transaction: {:?}", e);
                    errors.push(format!("Error processing transaction {:?}", signature));
                    continue;
                }
            };
        }

        Ok((signatures, errors))
    }
}

fn bin_counter_to_tag(counter: Option<i32>, bins: &[i32]) -> String {
    if counter.is_none() {
        return "none".to_string();
    }
    let counter = counter.unwrap();

    // Iterate through the bins vector to find the appropriate bin
    let mut bin_start = "-inf".to_string();
    let mut bin_end = "inf".to_string();
    for bin in bins.iter().rev() {
        if counter >= *bin {
            bin_start = bin.to_string();
            break;
        }

        bin_end = bin.to_string();
    }
    format!("{}_{}", bin_start, bin_end)
}

#[test]
fn test_bin_counter() {
    let bins = vec![0, 1, 2, 5, 10, 25];
    assert_eq!(bin_counter_to_tag(None, &bins), "none");
    assert_eq!(bin_counter_to_tag(Some(-100), &bins), "-inf_0");
    assert_eq!(bin_counter_to_tag(Some(0), &bins), "0_1");
    assert_eq!(bin_counter_to_tag(Some(1), &bins), "1_2");
    assert_eq!(bin_counter_to_tag(Some(2), &bins), "2_5");
    assert_eq!(bin_counter_to_tag(Some(3), &bins), "2_5");
    assert_eq!(bin_counter_to_tag(Some(17), &bins), "10_25");
    assert_eq!(bin_counter_to_tag(Some(34), &bins), "25_inf");
}
