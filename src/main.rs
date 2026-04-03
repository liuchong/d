use clap::Parser;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::exit;
use tracing::{error, info};

const DEFAULT_HOST: &str = "localhost";
const DEFAULT_PORT: u16 = 8080;

#[derive(Parser, Debug)]
#[command(
    name = "d",
    version,
    about = "D is a simple standalone httpd",
    author = "Liu Chong",
    long_about = None
)]
struct Cli {
    /// Set listening host
    #[arg(
        short = 'H',
        long,
        value_name = "HOST",
        default_value = DEFAULT_HOST,
        env = "D_HOST"
    )]
    host: String,

    /// Set listening port
    #[arg(
        short,
        long,
        value_name = "PORT",
        default_value_t = DEFAULT_PORT,
        env = "D_PORT"
    )]
    port: u16,

    /// Set root directory to serve
    #[arg(
        short,
        long,
        value_name = "PATH",
        default_value = ".",
        env = "D_ROOT"
    )]
    root: String,

    /// Set log level (trace, debug, info, warn, error)
    #[arg(
        short,
        long,
        value_name = "LEVEL",
        default_value = "info",
        env = "RUST_LOG"
    )]
    log: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Initialize tracing subscriber
    init_tracing(&cli.log);

    // Parse socket address
    let addr: SocketAddr = match format!("{}:{}", cli.host, cli.port)
        .to_socket_addrs()
    {
        Ok(mut addrs) => match addrs.next() {
            Some(a) => a,
            None => {
                error!("Failed to resolve address '{}:{}'", cli.host, cli.port);
                exit(1);
            }
        },
        Err(e) => {
            error!(
                "Failed to parse address '{}:{}': {}",
                cli.host, cli.port, e
            );
            exit(1);
        }
    };

    // Resolve root path
    let root = match std::fs::canonicalize(&cli.root) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => {
            error!("Invalid root path '{}': {}", cli.root, e);
            exit(1);
        }
    };

    info!("D HTTP Server starting...");
    info!("Configuration:");
    info!("  Host: {}", cli.host);
    info!("  Port: {}", cli.port);
    info!("  Root: {}", root);
    info!("  Log:  {}", cli.log);

    d::start(&addr, &root).await;
}

fn init_tracing(level: &str) {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_new(format!("d={}", level))
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_timer(tracing_subscriber::fmt::time::time())
                .compact(),
        )
        .with(filter)
        .init();
}
