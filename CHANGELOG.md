# Changelog

## [Unreleased]

### Added
- New RPC method `sendTransactionBundle` for sending multiple transactions serially
- Transaction bundle executor with blockhash validation
- Blockhash cache to track valid blockhashes
- New metrics for transaction bundles and blockhash validation
- Command-line tool for testing transaction bundles
- Support for devnet and testnet RPC endpoints
- Environment variable configuration for RPC and GRPC endpoints

### Changed
- Improved transaction validation by checking blockhashes before sending
- Enhanced error handling in transaction bundle execution
- Updated documentation with detailed usage examples
- Optimized bandwidth usage by validating blockhashes

### Fixed
- Fixed blockhash validation in transaction bundle execution
- Improved error handling for failed transactions
- Fixed metrics collection for transaction bundles

## [0.1.0] - Initial Release

### Added
- Basic transaction sending functionality
- RPC server implementation
- GRPC Geyser integration
- Transaction store for deduplication
- Basic metrics and logging

## Detailed Changes

### New RPC Method: `sendTransactionBundle`
- Added new method to `AtlasTxnSender` trait in [`src/rpc_server.rs`](src/rpc_server.rs)
- Accepts an array of transactions, parameters, and optional request metadata
- Returns a vector of transaction signatures
- Implements serial execution with proper error handling

### Transaction Bundle Executor
- Created new `TransactionBundleExecutor` struct in [`src/transaction_bundle.rs`](src/transaction_bundle.rs)
- Implements serial execution of transactions
- Validates blockhashes before sending transactions
- Maintains a cache of recent blockhashes
- Provides proper metrics and error handling

### Blockhash Validation
- Added blockhash cache to track valid blockhashes in [`src/transaction_bundle.rs`](src/transaction_bundle.rs)
- Validates blockhashes before sending transactions
- Skips transactions with expired blockhashes
- Updates cache when new blocks are received in [`src/grpc_geyser.rs`](src/grpc_geyser.rs)
- Provides metrics for invalid blockhashes

### Integration with Existing Components
- Uses existing transaction store for deduplication
- Uses existing transaction sender for reliable submission
- Uses existing RPC client for transaction confirmation
- Maintains proper metrics and logging

### Command-line Tool
- Added new `send_bundle` binary for testing transaction bundles
- Supports creating accounts, airdropping SOL, and sending transaction bundles
- Configurable RPC endpoints for different networks (devnet, testnet)
- Environment variable configuration for service settings

### Metrics
New metrics added:
- `send_transaction_bundle`: Count of bundle submissions
- `send_transaction_in_bundle`: Count of individual transactions in bundles
- `confirmed_transaction_in_bundle`: Count of confirmed transactions
- `error_transaction_in_bundle`: Count of failed transactions
- `invalid_blockhash`: Count of transactions with expired blockhashes

### Environment Variables
- `RPC_URL`: RPC url used to fetch next leaders with `getSlotLeaders`
- `GRPC_URL`: Yellowstone GRPC Geyser url for streaming slots and blocks
- `X_TOKEN`: Token used to authenticate with the GRPC url
- `TPU_CONNECTION_POOL_SIZE`: Number of leaders to cache connections to
- `NUM_LEADERS`: Number of leaders to send transactions to
- `LEADER_OFFSET`: Offset of the leader schedule
- `IDENTITY_KEYPAIR_FILE`: Path to the keypair file
- `PORT`: Port to run the service on (default: 4040)

### Dependencies
- Added `dashmap` for concurrent blockhash cache
- Added `anyhow` for error handling
- Added `clap` for command-line argument parsing
- Added `tracing` for structured logging

### Documentation
- Added detailed README with usage examples
- Added command-line tool documentation
- Added environment variable documentation
- Added metrics documentation 