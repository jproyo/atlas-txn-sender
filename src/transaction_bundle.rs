use std::sync::Arc;
use tracing::{error, info};

use crate::{
    solana_rpc::SolanaRpc,
    transaction_store::{TransactionData, TransactionStore},
    txn_sender::TxnSender,
};

pub struct TransactionBundleExecutor {
    txn_sender: Arc<dyn TxnSender>,
    transaction_store: Arc<dyn TransactionStore>,
    solana_rpc: Arc<dyn SolanaRpc>,
}

impl TransactionBundleExecutor {
    pub fn new(
        txn_sender: Arc<dyn TxnSender>,
        transaction_store: Arc<dyn TransactionStore>,
        solana_rpc: Arc<dyn SolanaRpc>,
    ) -> Self {
        Self {
            txn_sender,
            transaction_store,
            solana_rpc,
        }
    }

    pub async fn execute_bundle(
        &self,
        transactions: Vec<TransactionData>,
    ) -> anyhow::Result<Vec<String>> {
        let mut signatures = Vec::new();

        for transaction in transactions {
            let signature = transaction.versioned_transaction.signatures[0].to_string();
            signatures.push(signature.clone());

            // Send transaction
            self.txn_sender.send_transaction(transaction.clone());

            // Wait for confirmation
            match self.solana_rpc.confirm_transaction(signature.clone()).await {
                Some(_) => {
                    self.transaction_store.add_transaction(transaction);
                    info!("Transaction {} confirmed successfully", signature);
                }
                None => {
                    error!("Transaction {} failed or timed out", signature);
                    return Err(anyhow::anyhow!(
                        "Error processing transaction {}",
                        signature
                    ));
                }
            }
        }

        Ok(signatures)
    }
}
