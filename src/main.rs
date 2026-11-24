//! Binary entrypoint for `d`.

use d_cli::ParseOutcome;
use std::process::exit;

#[tokio::main]
async fn main() {
    let cfg = match d_cli::parse_from_env() {
        ParseOutcome::Run(cfg) => cfg,
        ParseOutcome::ExitOk(msg) => {
            print!("{msg}");
            return;
        }
        ParseOutcome::ExitErr(err) => {
            eprintln!("Error: {err}");
            exit(2);
        }
    };

    d_cli::init_log_level(cfg.log_level);

    let root_str = cfg.root.to_string_lossy().to_string();
    d_cli::log_info("D HTTP Server starting...");
    d_cli::log_info("Configuration:");
    d_cli::log_info(&format!("  Host: {}", cfg.host));
    d_cli::log_info(&format!("  Port: {}", cfg.port));
    d_cli::log_info(&format!("  Root: {root_str}"));
    d_cli::log_info(&format!("  Log:  {}", cfg.log_level.as_str()));
    d_cli::log_info(&format!(
        "  Hidden files: {}",
        if cfg.allow_hidden {
            "allowed"
        } else {
            "disabled"
        }
    ));
    d_cli::log_info(&format!(
        "  CORS: {}",
        if cfg.cors { "permissive" } else { "disabled" }
    ));

    d::start_with_options(
        &cfg.addr,
        &root_str,
        d::Options {
            allow_hidden: cfg.allow_hidden,
            cors: cfg.cors,
        },
    )
    .await;
}
