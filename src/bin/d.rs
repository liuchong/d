use clap::{App, Arg};
use log::error;
use std::env;
use std::net::ToSocketAddrs;
use std::process::exit;

const DEFAULT_HOST: &str = "localhost";
const DEFAULT_PORT: &str = "8080";

fn main() {
    let matches = App::new("d")
        .version("0.0.1")
        .about("D is a simple standalone httpd")
        .author("Liu Chong")
        .arg(
            Arg::with_name("host")
                .short("H")
                .long("host")
                .value_name("HOST")
                .help("Set listening host, default `localhost`"),
        )
        .arg(
            Arg::with_name("port")
                .short("p")
                .long("port")
                .value_name("PORT")
                .help("Set listening port, default `8080`"),
        )
        .arg(
            Arg::with_name("directory")
                .short("d")
                .long("root")
                .value_name("PATH TO ROOT")
                .help("Set root of server, default `current directory`"),
        )
        .arg(
            Arg::with_name("log")
                .short("l")
                .long("log")
                .value_name("LOG LEVEL")
                .help("Set rust log level, default `info`"),
        )
        .get_matches();

    let rust_log = matches.value_of("log").unwrap_or("info");
    env::set_var("RUST_LOG", rust_log);
    pretty_env_logger::init();

    let host = matches.value_of("host").unwrap_or(DEFAULT_HOST);
    let port = matches.value_of("port").unwrap_or(DEFAULT_PORT);
    let addr = format!("{}:{}", host, port)
        .to_socket_addrs()
        .unwrap_or_else(|_| {
            error!("Failed to parse socket addr from {}:{}", host, port);
            exit(1);
        })
        .next()
        .unwrap_or_else(|| {
            error!("Failed to get socket addr");
            exit(1);
        });

    let path = match matches.value_of("directory") {
        Some(d) => d.to_string(),
        None => current_dir(),
    };

    d::start(&addr, &path);
}

fn current_dir() -> String {
    env::current_dir()
        .unwrap_or_else(|_| {
            error!("Cannot get current directory");
            exit(1);
        })
        .to_str()
        .unwrap_or_else(|| {
            error!("Cannot get str of current directory");
            exit(1);
        })
        .to_string()
}
