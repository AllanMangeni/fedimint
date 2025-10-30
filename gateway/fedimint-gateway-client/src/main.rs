#![deny(clippy::pedantic, clippy::nursery)]

mod config_commands;
mod ecash_commands;
mod general_commands;
mod lightning_commands;
mod onchain_commands;

use clap::{CommandFactory, Parser, Subcommand};
use config_commands::ConfigCommands;
use ecash_commands::EcashCommands;
use fedimint_core::util::SafeUrl;
use fedimint_gateway_client::GatewayRpcClient;
use fedimint_logging::TracingSetup;
use general_commands::GeneralCommands;
use lightning_commands::LightningCommands;
use onchain_commands::OnchainCommands;
use serde::Serialize;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The address of the gateway webserver
    #[clap(long, default_value = "http://127.0.0.1:80")]
    address: SafeUrl,

    #[command(subcommand)]
    command: Commands,

    #[clap(long)]
    rpcpassword: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(flatten)]
    General(GeneralCommands),
    #[command(subcommand)]
    Lightning(LightningCommands),
    #[command(subcommand)]
    Ecash(EcashCommands),
    #[command(subcommand)]
    Onchain(OnchainCommands),
    #[command(subcommand)]
    Cfg(ConfigCommands),
    Completion {
        shell: clap_complete::Shell,
    },
}

#[tokio::main]
async fn main() {
    if let Err(err) = TracingSetup::default().init() {
        eprintln!("Failed to initialize logging: {err}");
        std::process::exit(1);
    }

    if let Err(err) = run().await {
        eprintln!("Error: {err}");
        let mut source = err.source();
        eprintln!("Caused by");
        while let Some(err) = source {
            eprintln!("    {err}");
            source = err.source();
        }
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = GatewayRpcClient::new(cli.address, cli.rpcpassword).await?;

    match cli.command {
        Commands::General(general_command) => general_command.handle(&client).await?,
        Commands::Lightning(lightning_command) => lightning_command.handle(&client).await?,
        Commands::Ecash(ecash_command) => ecash_command.handle(&client).await?,
        Commands::Onchain(onchain_command) => onchain_command.handle(&client).await?,
        Commands::Cfg(config_commands) => config_commands.handle(&client).await?,
        Commands::Completion { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "gateway-cli",
                &mut std::io::stdout(),
            );
        }
    }

    Ok(())
}

fn print_response<T: Serialize>(val: T) {
    println!(
        "{}",
        serde_json::to_string_pretty(&val).expect("Cannot serialize")
    );
}
