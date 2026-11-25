//! Binary entrypoint for `d-minimal`.

#[cfg(feature = "mcp")]
fn attach_control(
    ctx: frame::Context,
    mcp: d_cli::McpCliConfig,
) -> frame::BoxFuture<'static, Result<(), frame::FrameError>> {
    d_mcp::attach_mcp(ctx, mcp)
}

#[cfg(not(feature = "mcp"))]
fn attach_control(
    _ctx: frame::Context,
    _mcp: d_cli::McpCliConfig,
) -> frame::BoxFuture<'static, Result<(), frame::FrameError>> {
    Box::pin(async {
        Err(frame::FrameError::new(
            frame::ErrorKind::PluginNotFound,
            "d-minimal was built without `mcp` support; rebuild with `--features mcp`",
        ))
    })
}

#[tokio::main]
async fn main() {
    d_daemon::run_cli_daemon_with_control(
        "D Minimal HTTP Server starting...",
        "d-minimal",
        d_minimal::build_spec,
        attach_control,
    )
    .await;
}
