//! Thin pure-std command-line, environment, and stderr logging entrypoint for `d`.

use std::env;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

const DEFAULT_HOST: &str = "localhost";
const DEFAULT_PORT: u16 = 8080;
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Logging verbosity threshold matching `RUST_LOG` / `-l`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl LogLevel {
    /// Parse a log level token (`error`, `warn`, `info`, `debug`, `trace`).
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let target = trimmed
            .rsplit(',')
            .next()
            .unwrap_or(trimmed)
            .split('=')
            .next_back()
            .unwrap_or(trimmed)
            .trim()
            .to_ascii_lowercase();
        match target.as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    /// Return the canonical lowercase level name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

static ACTIVE_LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);
static LOG_TO_STDERR: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Configure the global stderr/stdout logging level threshold.
pub fn init_log_level(level: LogLevel) {
    ACTIVE_LOG_LEVEL.store(level as u8, Ordering::Release);
}

/// Return the currently configured global logging level threshold.
pub fn current_log_level() -> LogLevel {
    match ACTIVE_LOG_LEVEL.load(Ordering::Acquire) {
        1 => LogLevel::Error,
        2 => LogLevel::Warn,
        4 => LogLevel::Debug,
        5 => LogLevel::Trace,
        _ => LogLevel::Info,
    }
}

/// Route informational logs to `stderr` instead of `stdout` (required for stdio MCP).
pub fn set_log_to_stderr(enabled: bool) {
    LOG_TO_STDERR.store(enabled, Ordering::Release);
}

fn should_log(level: LogLevel) -> bool {
    (level as u8) <= ACTIVE_LOG_LEVEL.load(Ordering::Acquire)
}

/// Emit an informational log line if `INFO` is enabled.
pub fn log_info(msg: &str) {
    if should_log(LogLevel::Info) {
        if LOG_TO_STDERR.load(Ordering::Acquire) {
            eprintln!("INFO {msg}");
        } else {
            println!("INFO {msg}");
        }
    }
}

/// Emit a warning log line if `WARN` is enabled.
pub fn log_warn(msg: &str) {
    if should_log(LogLevel::Warn) {
        eprintln!("WARN {msg}");
    }
}

/// Emit an error log line if `ERROR` is enabled.
pub fn log_error(msg: &str) {
    if should_log(LogLevel::Error) {
        eprintln!("ERROR {msg}");
    }
}

/// Optional Model Context Protocol (MCP) server flags resolved by the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCliConfig {
    pub stdio: bool,
    pub http_addr: Option<SocketAddr>,
    pub http_path: String,
    pub auth_token: Option<String>,
    pub read_only: bool,
    pub allow_shutdown: bool,
}

impl Default for McpCliConfig {
    fn default() -> Self {
        Self {
            stdio: false,
            http_addr: None,
            http_path: "/mcp".to_string(),
            auth_token: None,
            read_only: false,
            allow_shutdown: false,
        }
    }
}

impl McpCliConfig {
    /// Return `true` when at least one MCP transport (stdio or HTTP) is enabled.
    pub fn is_enabled(&self) -> bool {
        self.stdio || self.http_addr.is_some()
    }
}

/// Resolved CLI configuration returned by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliConfig {
    pub host: String,
    pub port: u16,
    pub root: PathBuf,
    pub log_level: LogLevel,
    pub allow_hidden: bool,
    pub cors: bool,
    pub no_http: bool,
    pub mcp: McpCliConfig,
    pub addr: SocketAddr,
}

/// Outcome of CLI argument evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    Run(CliConfig),
    ExitOk(String),
    ExitErr(String),
}

fn help_text() -> String {
    format!(
        "D is a simple standalone httpd (v{VERSION})\n\n\
         USAGE:\n    d [OPTIONS]\n\n\
         OPTIONS:\n\
             -H, --host <HOST>           Set listening host [env: D_HOST] [default: {DEFAULT_HOST}]\n\
             -p, --port <PORT>           Set listening port [env: D_PORT] [default: {DEFAULT_PORT}]\n\
             -r, --root <PATH>           Set root directory to serve [env: D_ROOT] [default: .]\n\
             -l, --log <LEVEL>           Set log level (trace, debug, info, warn, error) [env: RUST_LOG] [default: info]\n\
                 --hidden                Show hidden files [env: D_SHOW_HIDDEN]\n\
                 --cors                  Enable permissive CORS [env: D_CORS]\n\
                 --no-http               Run in-memory HTTP router without binding TCP listener initially [env: D_NO_HTTP]\n\
                 --mcp-stdio             Enable stdio MCP control server (routes logs to stderr) [env: D_MCP_STDIO]\n\
                 --mcp-http <ADDR>       Enable HTTP MCP control server on host:port [env: D_MCP_HTTP]\n\
                 --mcp-path <PATH>       HTTP MCP endpoint path [env: D_MCP_PATH] [default: /mcp]\n\
                 --mcp-token <TOKEN>     Bearer token required for HTTP MCP endpoint [env: D_MCP_TOKEN]\n\
                 --mcp-read-only         Restrict MCP server to read-only inspection & probing [env: D_MCP_READ_ONLY]\n\
                 --mcp-allow-shutdown    Allow MCP d_shutdown tool [env: D_MCP_ALLOW_SHUTDOWN]\n\
             -h, --help                  Print help\n\
             -V, --version               Print version\n"
    )
}

fn parse_bool_env(val: &str) -> bool {
    matches!(
        val.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn parse_host_port_addr(raw: &str) -> Result<SocketAddr, String> {
    let trimmed = raw.trim();
    if let Ok(addr) = trimmed.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let Some((host, port_str)) = trimmed.rsplit_once(':') else {
        return Err(format!(
            "Invalid MCP HTTP address '{raw}': expected <host>:<port>"
        ));
    };
    let port: u16 = port_str
        .parse()
        .map_err(|e| format!("Invalid MCP HTTP port in '{raw}': {e}"))?;
    resolve_socket_addr(host, port)
}

/// Parse CLI arguments and environment variables using injectable sources.
pub fn parse_args<I, S, E>(args: I, env_fn: E) -> ParseOutcome
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
    E: Fn(&str) -> Option<String>,
{
    let mut host = env_fn("D_HOST")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_HOST.to_string());

    let mut port: u16 = match env_fn("D_PORT").filter(|s| !s.is_empty()) {
        Some(raw) => match raw.parse() {
            Ok(p) => p,
            Err(e) => {
                return ParseOutcome::ExitErr(format!(
                    "Invalid D_PORT '{raw}': {e}"
                ));
            }
        },
        None => DEFAULT_PORT,
    };

    let mut root_raw = env_fn("D_ROOT")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ".".to_string());

    let mut log_raw = env_fn("RUST_LOG")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "info".to_string());

    let mut allow_hidden =
        env_fn("D_SHOW_HIDDEN").is_some_and(|v| parse_bool_env(&v));
    let mut cors = env_fn("D_CORS").is_some_and(|v| parse_bool_env(&v));
    let mut no_http = env_fn("D_NO_HTTP").is_some_and(|v| parse_bool_env(&v));
    let mut mcp = McpCliConfig {
        stdio: env_fn("D_MCP_STDIO").is_some_and(|v| parse_bool_env(&v)),
        http_addr: match env_fn("D_MCP_HTTP").filter(|s| !s.trim().is_empty()) {
            Some(raw) => match parse_host_port_addr(&raw) {
                Ok(addr) => Some(addr),
                Err(msg) => return ParseOutcome::ExitErr(msg),
            },
            None => None,
        },
        http_path: env_fn("D_MCP_PATH")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/mcp".to_string()),
        auth_token: env_fn("D_MCP_TOKEN").filter(|s| !s.is_empty()),
        read_only: env_fn("D_MCP_READ_ONLY")
            .is_some_and(|v| parse_bool_env(&v)),
        allow_shutdown: env_fn("D_MCP_ALLOW_SHUTDOWN")
            .is_some_and(|v| parse_bool_env(&v)),
    };

    let mut iter = args.into_iter().skip(1);
    while let Some(arg) = iter.next() {
        let s = arg.as_ref();
        match s {
            "-h" | "--help" => return ParseOutcome::ExitOk(help_text()),
            "-V" | "--version" => {
                return ParseOutcome::ExitOk(format!("d {VERSION}\n"));
            }
            "--hidden" => allow_hidden = true,
            "--cors" => cors = true,
            "--no-http" => no_http = true,
            "--mcp-stdio" => mcp.stdio = true,
            "--mcp-read-only" => mcp.read_only = true,
            "--mcp-allow-shutdown" => mcp.allow_shutdown = true,
            "-H" | "--host" => {
                let Some(val) = iter.next() else {
                    return ParseOutcome::ExitErr(format!(
                        "Missing value for {s}"
                    ));
                };
                host = val.as_ref().to_string();
            }
            "-p" | "--port" => {
                let Some(val) = iter.next() else {
                    return ParseOutcome::ExitErr(format!(
                        "Missing value for {s}"
                    ));
                };
                port = match val.as_ref().parse() {
                    Ok(p) => p,
                    Err(e) => {
                        return ParseOutcome::ExitErr(format!(
                            "Invalid port '{}': {e}",
                            val.as_ref()
                        ));
                    }
                };
            }
            "-r" | "--root" => {
                let Some(val) = iter.next() else {
                    return ParseOutcome::ExitErr(format!(
                        "Missing value for {s}"
                    ));
                };
                root_raw = val.as_ref().to_string();
            }
            "-l" | "--log" => {
                let Some(val) = iter.next() else {
                    return ParseOutcome::ExitErr(format!(
                        "Missing value for {s}"
                    ));
                };
                log_raw = val.as_ref().to_string();
            }
            "--mcp-http" => {
                let Some(val) = iter.next() else {
                    return ParseOutcome::ExitErr(format!(
                        "Missing value for {s}"
                    ));
                };
                mcp.http_addr = match parse_host_port_addr(val.as_ref()) {
                    Ok(a) => Some(a),
                    Err(msg) => return ParseOutcome::ExitErr(msg),
                };
            }
            "--mcp-path" => {
                let Some(val) = iter.next() else {
                    return ParseOutcome::ExitErr(format!(
                        "Missing value for {s}"
                    ));
                };
                mcp.http_path = val.as_ref().to_string();
            }
            "--mcp-token" => {
                let Some(val) = iter.next() else {
                    return ParseOutcome::ExitErr(format!(
                        "Missing value for {s}"
                    ));
                };
                let tok = val.as_ref().to_string();
                if tok.is_empty() {
                    return ParseOutcome::ExitErr(
                        "Invalid empty --mcp-token".to_string(),
                    );
                }
                mcp.auth_token = Some(tok);
            }
            _ if s.starts_with("--host=") => {
                host = s["--host=".len()..].to_string();
            }
            _ if s.starts_with("--port=") => {
                let raw = &s["--port=".len()..];
                port = match raw.parse() {
                    Ok(p) => p,
                    Err(e) => {
                        return ParseOutcome::ExitErr(format!(
                            "Invalid port '{raw}': {e}"
                        ));
                    }
                };
            }
            _ if s.starts_with("--root=") => {
                root_raw = s["--root=".len()..].to_string();
            }
            _ if s.starts_with("--log=") => {
                log_raw = s["--log=".len()..].to_string();
            }
            _ if s.starts_with("--mcp-http=") => {
                let raw = &s["--mcp-http=".len()..];
                mcp.http_addr = match parse_host_port_addr(raw) {
                    Ok(a) => Some(a),
                    Err(msg) => return ParseOutcome::ExitErr(msg),
                };
            }
            _ if s.starts_with("--mcp-path=") => {
                mcp.http_path = s["--mcp-path=".len()..].to_string();
            }
            _ if s.starts_with("--mcp-token=") => {
                let tok = &s["--mcp-token=".len()..];
                if tok.is_empty() {
                    return ParseOutcome::ExitErr(
                        "Invalid empty --mcp-token".to_string(),
                    );
                }
                mcp.auth_token = Some(tok.to_string());
            }
            _ => {
                return ParseOutcome::ExitErr(format!(
                    "Unknown argument: {s}\nUse --help for usage."
                ));
            }
        }
    }

    if !mcp.http_path.starts_with('/') {
        return ParseOutcome::ExitErr(format!(
            "Invalid MCP path '{}': must start with '/'",
            mcp.http_path
        ));
    }

    let log_level = LogLevel::parse(&log_raw).unwrap_or(LogLevel::Info);

    let addr = match resolve_socket_addr(&host, port) {
        Ok(a) => a,
        Err(msg) => return ParseOutcome::ExitErr(msg),
    };

    let root = match std::fs::canonicalize(&root_raw) {
        Ok(p) => p,
        Err(e) => {
            return ParseOutcome::ExitErr(format!(
                "Invalid root path '{root_raw}': {e}"
            ));
        }
    };

    ParseOutcome::Run(CliConfig {
        host,
        port,
        root,
        log_level,
        allow_hidden,
        cors,
        no_http,
        mcp,
        addr,
    })
}

fn resolve_socket_addr(host: &str, port: u16) -> Result<SocketAddr, String> {
    let addrs: Vec<SocketAddr> = format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| format!("Failed to parse address '{host}:{port}': {e}"))?
        .collect();
    if let Some(v4) = addrs.iter().copied().find(|a| a.is_ipv4()) {
        return Ok(v4);
    }
    addrs
        .into_iter()
        .next()
        .ok_or_else(|| format!("Failed to resolve address '{host}:{port}'"))
}

/// Parse arguments from process `std::env::args` and `std::env::var`.
pub fn parse_from_env() -> ParseOutcome {
    parse_args(env::args(), |k| env::var(k).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parses_cli_flags_overriding_env() {
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().into_owned();
        let mut envs = HashMap::new();
        envs.insert("D_PORT", "9090".to_string());
        envs.insert("D_CORS", "true".to_string());
        envs.insert("D_SHOW_HIDDEN", "1".to_string());
        envs.insert("D_ROOT", tmp_str.clone());

        let out = parse_args(
            ["d", "-H", "127.0.0.1", "-p", "18080", "-l", "debug"],
            |k| envs.get(k).cloned(),
        );
        let ParseOutcome::Run(cfg) = out else {
            panic!("expected Run, got: {out:?}");
        };
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 18080);
        assert_eq!(cfg.log_level, LogLevel::Debug);
        assert!(cfg.cors);
        assert!(cfg.allow_hidden);
        assert_eq!(cfg.addr.port(), 18080);
    }

    #[test]
    fn env_defaults_and_help_exit() {
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().into_owned();
        let out = parse_args(["d", "--root", &tmp_str, "--hidden"], |_| None);
        let ParseOutcome::Run(cfg) = out else {
            panic!("expected Run");
        };
        assert_eq!(cfg.port, 8080);
        assert!(cfg.allow_hidden);
        assert!(!cfg.cors);

        assert!(matches!(
            parse_args(["d", "--help"], |_| None),
            ParseOutcome::ExitOk(t) if t.contains("--cors") && t.contains("--mcp-stdio")
        ));
    }

    #[test]
    fn parses_mcp_cli_and_env_options() {
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().into_owned();
        let mut envs = HashMap::new();
        envs.insert("D_ROOT", tmp_str);
        envs.insert("D_MCP_STDIO", "1".to_string());
        envs.insert("D_MCP_HTTP", "127.0.0.1:18888".to_string());
        envs.insert("D_MCP_PATH", "/control".to_string());
        envs.insert("D_MCP_TOKEN", "tok-123".to_string());
        envs.insert("D_MCP_READ_ONLY", "true".to_string());
        envs.insert("D_NO_HTTP", "yes".to_string());

        let out =
            parse_args(["d", "--mcp-allow-shutdown"], |k| envs.get(k).cloned());
        let ParseOutcome::Run(cfg) = out else {
            panic!("expected Run, got: {out:?}");
        };
        assert!(cfg.no_http);
        assert!(cfg.mcp.is_enabled());
        assert!(cfg.mcp.stdio);
        assert_eq!(cfg.mcp.http_addr, Some("127.0.0.1:18888".parse().unwrap()));
        assert_eq!(cfg.mcp.http_path, "/control");
        assert_eq!(cfg.mcp.auth_token.as_deref(), Some("tok-123"));
        assert!(cfg.mcp.read_only);
        assert!(cfg.mcp.allow_shutdown);

        assert!(matches!(
            parse_args(["d", "--mcp-path", "no-leading-slash"], |_| None),
            ParseOutcome::ExitErr(msg) if msg.contains("must start with '/'")
        ));
    }
}
