use clap::{Parser, Subcommand};
use std::net::ToSocketAddrs;
use tracing::info;

#[derive(Parser)]
#[command(name = "d")]
#[command(about = "D - AI Daemon with HTTP server and CLI chat")]
#[command(version)]
struct Cli {
    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run HTTP server mode
    Server {
        /// Host to bind
        #[arg(short = 'H', long, default_value = "localhost")]
        host: String,
        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,
        /// Root directory to serve
        #[arg(short = 'r', long, default_value = ".")]
        root: String,
    },
    /// Run CLI chat mode
    Chat {
        /// Start new session
        #[arg(short, long)]
        new: bool,
        /// Yolo mode (auto-approve tools)
        #[arg(long)]
        yolo: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    
    // Initialize tracing with log level from CLI
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::new(&cli.log_level)
                .add_directive(format!("{}={}", env!("CARGO_PKG_NAME"), cli.log_level).parse()?)
        )
        .init();

    match cli.command {
        Some(Commands::Server { host, port, root }) => {
            let addr = format!("{}:{}", host, port).to_socket_addrs()?.next()
                .ok_or_else(|| anyhow::anyhow!("Cannot resolve address: {}:{}", host, port))?;
            info!("Starting HTTP server on http://{}", addr);
            info!("Serving directory: {}", root);
            http::start(&addr, &root, false).await;
        }
        Some(Commands::Chat { new, yolo }) => {
            info!("Starting CLI chat mode");
            if new {
                info!("Creating new session");
            }
            if yolo {
                info!("Yolo mode enabled");
            }
            // TODO: Start CLI chat
            println!("CLI chat mode - not yet implemented");
        }
        None => {
            // Default: start HTTP server on localhost:8080
            let addr = ("localhost", 8080).to_socket_addrs()?.next()
                .ok_or_else(|| anyhow::anyhow!("Cannot resolve localhost:8080"))?;
            info!("Starting HTTP server on http://{}", addr);
            info!("Serving directory: .");
            http::start(&addr, ".", false).await;
        }
    }

    Ok(())
}
