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
- Added confirmed transactions tracking in TransactionStore
- Added methods to add, get, and remove confirmed transactions
- Added transaction confirmation status tracking in TxnSender
- Added sequential transaction processing in TransactionBundleExecutor
- Added blockhash cache for transaction validation

### Changed
- Improved transaction validation by checking blockhashes before sending
- Enhanced error handling in transaction bundle execution
- Updated documentation with detailed usage examples
- Optimized bandwidth usage by validating blockhashes
- Improved blockhash handling to update each transaction with the latest blockhash
- Enhanced error reporting to return both successful signatures and error messages
- Modified transaction bundle execution to continue processing on individual transaction failures
- Modified transaction confirmation flow to use TransactionStore
- Updated transaction bundle processing to handle confirmations sequentially
- Improved error handling and logging for transaction confirmations
- Removed direct RPC dependency from TransactionBundleExecutor
- Optimized transaction confirmation polling interval
- Refactored codebase into modular structure:
  - `server`: RPC server implementation
  - `application`: Core business logic with transaction processing
  - `storage`: Transaction store implementation
  - `infrastructure`: Infrastructure components (txn_sender, transaction_bundle, solana_rpc, grpc_geyser, leader_tracker)
  - `metrics`: Metrics implementation
- Moved transaction bundle logic from `TransactionBundleExecutor` to `TxnSender`:
  - Integrated bundle functionality directly into `TxnSender` trait
  - Removed separate `TransactionBundleExecutor` struct
  - Simplified transaction bundle processing flow
  - Improved error handling and metrics collection
- Converted `send_transaction_bundle` to synchronous function:
  - Removed async/await from bundle processing
  - Simplified transaction confirmation waiting logic
  - Improved performance by reducing async overhead
  - Maintained same functionality with simpler implementation

### Fixed
- Fixed blockhash validation in transaction bundle execution
- Improved error handling for failed transactions
- Fixed metrics collection for transaction bundles
- Fixed blockhash cache initialization and maintenance
- Fixed transaction signature handling in confirmation process
- Fixed transaction cleanup after confirmation
- Fixed error handling in transaction confirmation loop

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
- Returns a tuple of successful signatures and error messages
- Implements serial execution with proper error handling

### Transaction Bundle Executor
- Created new `TransactionBundleExecutor` struct in [`src/transaction_bundle.rs`](src/transaction_bundle.rs)
- Implements serial execution of transactions
- Updates each transaction with the latest blockhash from cache
- Maintains a cache of recent blockhashes
- Provides proper metrics and error handling
- Continues processing on individual transaction failures

### Blockhash Validation
- Added blockhash cache to track valid blockhashes in [`src/transaction_bundle.rs`](src/transaction_bundle.rs)
- Updates transactions with latest blockhash before sending
- Uses GRPC stream to maintain blockhash cache
- Provides metrics for invalid blockhashes
- Improved error reporting for blockhash-related issues

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
- `no_valid_blockhash`: Count of times no valid blockhash was available

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