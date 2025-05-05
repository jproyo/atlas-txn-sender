use anyhow::Result;
use clap::{Parser, Subcommand};
use jsonrpsee::{core::client::ClientT, http_client::HttpClientBuilder, rpc_params};
use serde::Deserialize;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use std::{fs, path::PathBuf};
use tracing::{error, info, Level};

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all(serialize = "camelCase", deserialize = "camelCase"))]
pub struct BundleResponse {
    pub signatures: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The Solana RPC URL to connect to (e.g., http://localhost:8899 for local validator, http://api.devnet.solana.com for devnet)
    #[arg(
        long,
        required = true,
        help = "The Solana RPC URL to connect to (e.g., http://localhost:8899 for local validator, http://api.devnet.solana.com for devnet)"
    )]
    solana_rpc_url: String,

    /// The directory where account keypairs will be stored
    #[arg(
        long,
        default_value = "accounts",
        help = "Directory where account keypairs will be stored"
    )]
    accounts_dir: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create new Solana accounts and save their keypairs to files
    CreateAccounts {
        /// Number of accounts to create
        #[arg(
            short,
            long,
            default_value = "2",
            help = "Number of accounts to create"
        )]
        count: usize,
    },

    /// Request an airdrop of SOL tokens to an account
    Airdrop {
        /// Path to the account file to receive the airdrop
        #[arg(
            short = 'f',
            long,
            help = "Path to the account file (e.g., accounts/account_0.json)"
        )]
        account: PathBuf,

        /// Amount of SOL to airdrop (in lamports, 1 SOL = 1_000_000_000 lamports)
        #[arg(
            short = 'a',
            long,
            default_value = "1000000000",
            help = "Amount in lamports (1 SOL = 1_000_000_000 lamports)"
        )]
        amount: u64,
    },

    /// Send a bundle of transactions to the transaction sender service
    SendBundle {
        /// The URL of the transaction sender service
        #[arg(
            short = 'u',
            long,
            default_value = "http://localhost:4040",
            help = "URL of the transaction sender service"
        )]
        url: String,

        /// The API key for authenticating with the transaction sender service
        #[arg(short = 'k', long, help = "API key for the transaction sender service")]
        api_key: String,

        /// Path to the sender account file
        #[arg(
            short = 'f',
            long,
            help = "Path to the sender account file (e.g., accounts/account_0.json)"
        )]
        from: PathBuf,

        /// Path to the receiver account file
        #[arg(
            short = 't',
            long,
            help = "Path to the receiver account file (e.g., accounts/account_1.json)"
        )]
        to: PathBuf,

        /// Number of transactions to include in the bundle
        #[arg(
            short = 'c',
            long,
            default_value = "1",
            help = "Number of transactions to send"
        )]
        count: usize,

        /// Amount of SOL to transfer in each transaction (in lamports)
        #[arg(
            short = 'a',
            long,
            default_value = "1000000",
            help = "Amount per transaction in lamports (1 SOL = 1_000_000_000 lamports)"
        )]
        amount: u64,
    },
}

fn load_keypair(path: &PathBuf) -> Result<Keypair> {
    let data = fs::read(path)?;
    Ok(Keypair::from_bytes(&data)?)
}

fn save_keypair(keypair: &Keypair, path: &PathBuf) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, keypair.to_bytes())?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    let args = Args::parse();

    match args.command {
        Commands::CreateAccounts { count } => {
            let output_dir = PathBuf::from(&args.accounts_dir);
            info!(
                "Creating {} accounts in directory: {}",
                count,
                output_dir.display()
            );
            fs::create_dir_all(&output_dir)?;

            for i in 0..count {
                let keypair = Keypair::new();
                let path = output_dir.join(format!("account_{}.json", i));
                save_keypair(&keypair, &path)?;
                info!("Created account {}: {}", i, keypair.pubkey());
            }
        }

        Commands::Airdrop { account, amount } => {
            info!("Airdropping {} lamports to account: {:?}", amount, account);
            let keypair = load_keypair(&account)?;
            let client = RpcClient::new_with_commitment(
                args.solana_rpc_url.to_string(),
                CommitmentConfig::confirmed(),
            );
            client.request_airdrop(&keypair.pubkey(), amount)?;
            info!("Airdrop successful!");
        }

        Commands::SendBundle {
            url,
            api_key,
            from,
            to,
            count,
            amount,
        } => {
            info!("Sending transaction bundle...");
            let from_keypair = load_keypair(&from)?;
            let to_keypair = load_keypair(&to)?;

            info!("From account: {}", from_keypair.pubkey());
            info!("To account: {}", to_keypair.pubkey());

            // Create RPC client to get recent blockhash
            let rpc_client = RpcClient::new_with_commitment(
                args.solana_rpc_url.to_string(),
                CommitmentConfig::confirmed(),
            );

            // Get rent-exempt minimum balance
            let rent_exempt_balance = 890880; // Minimum balance for rent exemption (0.00089088 SOL)

            // Create HTTP client
            let client = HttpClientBuilder::default().build(url)?;

            // Create transactions
            let mut transactions = Vec::new();
            for _ in 0..count {
                // Get a new blockhash for each transaction
                let recent_blockhash = rpc_client
                    .get_latest_blockhash()
                    .map_err(|e| anyhow::anyhow!("Failed to get recent blockhash: {}", e))?;

                // Add compute budget instructions for prioritization fee
                let compute_unit_price = 100_000; // Set a high priority fee (in micro-lamports)
                let compute_unit_limit = 200_000; // Set compute unit limit

                let instructions = vec![
                    ComputeBudgetInstruction::set_compute_unit_price(compute_unit_price),
                    ComputeBudgetInstruction::set_compute_unit_limit(compute_unit_limit),
                    system_instruction::transfer(
                        &from_keypair.pubkey(),
                        &to_keypair.pubkey(),
                        amount + rent_exempt_balance, // Add rent-exempt balance to the transfer amount
                    ),
                ];

                let transaction = Transaction::new_signed_with_payer(
                    &instructions,
                    Some(&from_keypair.pubkey()),
                    &[&from_keypair],
                    recent_blockhash,
                );

                // Convert transaction to base58
                let encoded = bs58::encode(bincode::serialize(&transaction)?).into_string();
                transactions.push(encoded);

                // Add a small delay between transactions to ensure different blockhashes
                if count > 1 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                }
            }

            // Prepare request metadata
            let request_metadata = serde_json::json!({
                "apiKey": api_key
            });

            // Send transaction bundle
            let params = rpc_params!(
                transactions,
                serde_json::json!({
                    "skipPreflight": true,
                    "encoding": "base58"
                }),
                request_metadata
            );

            let response: BundleResponse = client
                .request("sendTransactionBundle", params)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send transaction bundle: {}", e))?;

            if response.signatures.is_empty() {
                info!("No signatures returned from the service. This might indicate an error.");
                if response.errors.is_empty() {
                    error!("Failed to send transaction bundle {:?}", response);
                } else {
                    error!("Service errors: {:?}", response.errors);
                }
            } else {
                info!("Transaction bundle sent successfully!");
                info!("Signatures: {:?}", response.signatures);
            }
        }
    }

    Ok(())
}
