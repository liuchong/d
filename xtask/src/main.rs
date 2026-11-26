//! Pure-`std` workspace governance CLI (`cargo xtask <check|info|test>`).

mod check;
mod graph;
mod info;
mod json;
mod publish;
mod test_cmd;

use graph::WorkspaceGraph;
use std::process::exit;

fn print_help() {
    println!(
        "cargo xtask <subcommand>\n\n\
         Subcommands:\n\
           check                         Verify all 9 workspace architecture rules\n\
           info                          Display brick counts, assembly matrix, and contract mappings\n\
           test [--base REV] [--dry-run] Run tests for transitive dependents of changed files\n\
           publish-d [--dry-run | --execute]  Bundle and publish single-crate `d` to crates.io"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        print_help();
        exit(2);
    };

    if matches!(cmd, "-h" | "--help" | "help") {
        print_help();
        return;
    }

    let graph = match WorkspaceGraph::load_live() {
        Ok(g) => g,
        Err(err) => {
            eprintln!("xtask error: {err}");
            exit(1);
        }
    };

    let code = match cmd {
        "check" => check::run_check(&graph),
        "info" => info::run_info(&graph),
        "test" => test_cmd::run_test(&graph, &args[1..]),
        "publish-d" => publish::run_publish_d(&graph, &args[1..]),
        other => {
            eprintln!("Unknown xtask subcommand: `{other}`");
            print_help();
            2
        }
    };
    if code != 0 {
        exit(code);
    }
}
