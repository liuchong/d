//! Binary entrypoint for `d`.

#[tokio::main]
async fn main() {
    d_daemon::run_cli_daemon("D HTTP Server starting...", d::build_spec).await;
}
