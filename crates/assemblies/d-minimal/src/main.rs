//! Binary entrypoint for `d-minimal`.

#[tokio::main]
async fn main() {
    d_daemon::run_cli_daemon(
        "D Minimal HTTP Server starting...",
        d_minimal::build_spec,
    )
    .await;
}
