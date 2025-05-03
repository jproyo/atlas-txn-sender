use std::env;
use std::net::UdpSocket;

use cadence::{BufferedUdpMetricSink, QueuingMetricSink, StatsdClient};
use cadence_macros::set_global_default;
use tracing::{error, info};

pub const ENCODE_ERROR: &str = "encode_error";
pub const SERVER_ERROR: &str = "server_error";
pub const GRPC_SUBSCRIBE_ERROR: &str = "grpc_subscribe_error";
pub const WSS_SUBSCRIBE_ACCEPT_ERROR: &str = "wss_subscribe_accept_error";
pub const GRPC_CLOSE_ERROR: &str = "grpc_close_error";
pub const WSS_SEND_ERROR: &str = "wss_send_error";
pub const ACTIVE_CONNECTIONS: &str = "active_connections";
pub const TRANSACTION_NOTIFICATION: &str = "transaction_notification";
pub const ACCOUNT_NOTIFICATION: &str = "account_notification";
pub const UNKNOWN_NOTIFICATION: &str = "unknown_notification";
pub const PING: &str = "ping";
pub const TRANSACTION_SUBSCRIBE_CLOSED: &str = "transaction_subscribe_closed";
pub const ACCOUNT_SUBSCRIBE_CLOSED: &str = "account_subscribe_closed";

pub fn new_metrics_client() {
    let uri = env::var("METRICS_URI").unwrap_or("127.0.0.1".to_string());
    let port = env::var("METRICS_PORT")
        .unwrap_or("7998".to_string())
        .parse::<u16>()
        .unwrap();
    info!("collecting metrics on: {}:{}", uri, port);
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.set_nonblocking(true).unwrap();

    let host = (uri, port);
    let udp_sink = BufferedUdpMetricSink::from(host, socket).unwrap();
    let queuing_sink = QueuingMetricSink::from(udp_sink);
    let builder = StatsdClient::builder("atlas_txn_sender", queuing_sink);
    let client = builder
        .with_error_handler(|e| error!("statsd metrics error: {}", e))
        .build();
    set_global_default(client);
}
