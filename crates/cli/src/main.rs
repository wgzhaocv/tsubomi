use anyhow::Result;
use clap::{Parser, Subcommand};
use tsubomi_shared::{Greeting, Health};

/// tsubomi command-line client.
#[derive(Parser)]
#[command(name = "tsubomi", version, about)]
struct Cli {
    /// Base URL of the tsubomi server.
    #[arg(
        long,
        env = "TSUBOMI_SERVER",
        default_value = "http://localhost:8080",
        global = true
    )]
    server: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch a greeting from the server.
    Hello,
    /// Check server health.
    Health,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();
    let base = cli.server.trim_end_matches('/');

    match cli.command {
        Command::Hello => {
            let greeting: Greeting = client
                .get(format!("{base}/api/hello"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            println!("{}", greeting.message);
        }
        Command::Health => {
            let health: Health = client
                .get(format!("{base}/api/health"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            println!("status: {}  version: {}", health.status, health.version);
        }
    }

    Ok(())
}
