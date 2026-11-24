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

/// Configure the global stderr/stdout logging level threshold.
pub fn init_log_level(level: LogLevel) {
    ACTIVE_LOG_LEVEL.store(level as u8, Ordering::Release);
}

fn should_log(level: LogLevel) -> bool {
    (level as u8) <= ACTIVE_LOG_LEVEL.load(Ordering::Acquire)
}

/// Emit an informational log line if `INFO` is enabled.
pub fn log_info(msg: &str) {
    if should_log(LogLevel::Info) {
        println!("INFO {msg}");
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

/// Resolved CLI configuration returned by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliConfig {
    pub host: String,
    pub port: u16,
    pub root: PathBuf,
    pub log_level: LogLevel,
    pub allow_hidden: bool,
    pub cors: bool,
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
             -H, --host <HOST>    Set listening host [env: D_HOST] [default: {DEFAULT_HOST}]\n\
             -p, --port <PORT>    Set listening port [env: D_PORT] [default: {DEFAULT_PORT}]\n\
             -r, --root <PATH>    Set root directory to serve [env: D_ROOT] [default: .]\n\
             -l, --log <LEVEL>    Set log level (trace, debug, info, warn, error) [env: RUST_LOG] [default: info]\n\
                 --hidden         Show hidden files [env: D_SHOW_HIDDEN]\n\
                 --cors           Enable permissive CORS [env: D_CORS]\n\
             -h, --help           Print help\n\
             -V, --version        Print version\n"
    )
}

fn parse_bool_env(val: &str) -> bool {
    matches!(
        val.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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
            _ => {
                return ParseOutcome::ExitErr(format!(
                    "Unknown argument: {s}\nUse --help for usage."
                ));
            }
        }
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
            ParseOutcome::ExitOk(t) if t.contains("--cors")
        ));
    }
}
