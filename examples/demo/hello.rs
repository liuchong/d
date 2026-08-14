use std::net::SocketAddr;

/// A tiny example to show off syntax highlighting in `d`.
#[tokio::main]
async fn main() {
    let addr: SocketAddr = "127.0.0.1:8080".parse().expect("valid address");
    println!("Serving files at http://{addr}");

    let languages = vec!["rust", "python", "go", "javascript"];
    for lang in &languages {
        println!("  - {lang}");
    }
}
