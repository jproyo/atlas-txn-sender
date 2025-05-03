use atlas_txn_sender::metrics::new_metrics_client;
use atlas_txn_sender::rpc_server::{AtlasTxnSenderImpl, AtlasTxnSenderServer};
use atlas_txn_sender::transaction_bundle::TransactionBundleExecutor;
use atlas_txn_sender::{grpc_geyser::GrpcGeyserImpl, leader_tracker::LeaderTrackerImpl};
use atlas_txn_sender::{transaction_store::TransactionStoreImpl, txn_sender::TxnSenderImpl};
use figment::{providers::Env, Figment};
use jsonrpsee::server::{middleware::ProxyGetRequestLayer, ServerBuilder};
use serde::Deserialize;
use solana_client::{connection_cache::ConnectionCache, rpc_client::RpcClient};
use solana_sdk::signature::{read_keypair_file, Keypair};
use std::{
    env,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

#[derive(Debug, Deserialize)]
struct AtlasTxnSenderEnv {
    identity_keypair_file: Option<String>,
    grpc_url: Option<String>,
    rpc_url: Option<String>,
    port: Option<u16>,
    tpu_connection_pool_size: Option<usize>,
    x_token: Option<String>,
    num_leaders: Option<usize>,
    leader_offset: Option<i64>,
    txn_sender_threads: Option<usize>,
    max_txn_send_retries: Option<usize>,
    txn_send_retry_interval: Option<usize>,
    max_retry_queue_size: Option<usize>,
}

// Defualt on RPC is 4
pub const DEFAULT_TPU_CONNECTION_POOL_SIZE: usize = 4;

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;
#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Init metrics/logging
    let env: AtlasTxnSenderEnv = Figment::from(Env::raw()).extract().unwrap();
    let env_filter = env::var("RUST_LOG").unwrap_or("info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .json()
        .init();
    new_metrics_client();

    let service_builder = tower::ServiceBuilder::new()
        // Proxy `GET /health` requests to internal `health` method.
        .layer(ProxyGetRequestLayer::new("/health", "health")?);
    let port = env.port.unwrap_or(4040);

    let server = ServerBuilder::default()
        .set_middleware(service_builder)
        .max_request_body_size(15_000_000)
        .max_connections(1_000_000)
        .build(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    let tpu_connection_pool_size = env
        .tpu_connection_pool_size
        .unwrap_or(DEFAULT_TPU_CONNECTION_POOL_SIZE);
    let connection_cache;
    if let Some(identity_keypair_file) = env.identity_keypair_file.clone() {
        let identity_keypair =
            read_keypair_file(identity_keypair_file).expect("keypair file must exist");
        connection_cache = Arc::new(ConnectionCache::new_with_client_options(
            "atlas-txn-sender",
            tpu_connection_pool_size,
            None, // created if none specified
            Some((&identity_keypair, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))),
            None, // not used as far as I can tell
        ));
    } else {
        let identity_keypair = Keypair::new();
        connection_cache = Arc::new(ConnectionCache::new_with_client_options(
            "atlas-txn-sender",
            tpu_connection_pool_size,
            None, // created if none specified
            Some((&identity_keypair, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))),
            None, // not used as far as I can tell
        ));
    }

    let transaction_store = Arc::new(TransactionStoreImpl::new());
    let solana_rpc = Arc::new(GrpcGeyserImpl::new(
        env.grpc_url.clone().unwrap(),
        env.x_token.clone(),
    ));
    let rpc_client = Arc::new(RpcClient::new(env.rpc_url.unwrap()));
    let num_leaders = env.num_leaders.unwrap_or(2);
    let leader_offset = env.leader_offset.unwrap_or(0);
    let leader_tracker = Arc::new(LeaderTrackerImpl::new(
        rpc_client,
        solana_rpc.clone(),
        num_leaders,
        leader_offset,
    ));
    let txn_send_retry_interval_seconds = env.txn_send_retry_interval.unwrap_or(2);
    let txn_sender = Arc::new(TxnSenderImpl::new(
        leader_tracker,
        transaction_store.clone(),
        connection_cache,
        solana_rpc.clone(),
        env.txn_sender_threads.unwrap_or(4),
        txn_send_retry_interval_seconds,
        env.max_retry_queue_size,
    ));
    let max_txn_send_retries = env.max_txn_send_retries.unwrap_or(5);
    let bundle_executor = Arc::new(TransactionBundleExecutor::new(
        txn_sender.clone(),
        transaction_store.clone(),
        solana_rpc.clone(),
    ));
    let atlas_txn_sender = AtlasTxnSenderImpl::new(
        txn_sender,
        transaction_store,
        max_txn_send_retries,
        bundle_executor,
    );
    let handle = server.start(atlas_txn_sender.into_rpc());
    handle.stopped().await;
    Ok(())
}
