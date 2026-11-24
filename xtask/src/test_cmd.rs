//! Incremental transitive-closure test runner (`cargo xtask test`).

use crate::graph::{DepKind, WorkspaceGraph};
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// Compute the reverse transitive closure of `seeds` within the workspace.
pub fn reverse_closure(
    graph: &WorkspaceGraph,
    seeds: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut rev: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, pkg) in &graph.packages {
        for dep in &pkg.deps {
            if !dep.external {
                rev.entry(dep.name.as_str())
                    .or_default()
                    .push(name.as_str());
            }
        }
    }

    let mut affected = BTreeSet::new();
    let mut stack: Vec<String> = seeds.iter().cloned().collect();
    while let Some(cur) = stack.pop() {
        if !graph.packages.contains_key(&cur) || !affected.insert(cur.clone()) {
            continue;
        }
        if let Some(dependents) = rev.get(cur.as_str()) {
            for dep in dependents {
                stack.push((*dep).to_string());
            }
        }
    }
    affected
}

/// Map repo-relative changed paths to the workspace packages they belong to.
pub fn map_changed_paths(
    graph: &WorkspaceGraph,
    changed_paths: &[String],
) -> BTreeSet<String> {
    let mut seeds = BTreeSet::new();
    for raw in changed_paths {
        let norm = raw.replace('\\', "/");
        let p = norm.trim_start_matches("./");
        if p.is_empty() {
            continue;
        }
        if matches!(p, "Cargo.toml" | "Cargo.lock" | "rustfmt.toml")
            || p.starts_with(".cargo/")
        {
            seeds.extend(graph.packages.keys().cloned());
            return seeds;
        }

        let mut best_match: Option<(usize, &str)> = None;
        for (name, pkg) in &graph.packages {
            let dir = pkg.manifest_dir.as_str();
            let matched = !dir.is_empty()
                && (p == dir
                    || p.strip_prefix(dir)
                        .is_some_and(|rest| rest.starts_with('/')));
            if matched && best_match.is_none_or(|(len, _)| dir.len() > len) {
                best_match = Some((dir.len(), name.as_str()));
            }
        }
        if let Some((_, name)) = best_match {
            seeds.insert(name.to_string());
        }
    }
    seeds
}

fn collect_changed_files(
    root: &std::path::Path,
    base: &str,
) -> Result<Vec<String>, String> {
    let diff_out = Command::new("git")
        .current_dir(root)
        .args(["diff", "--name-only", base])
        .output()
        .map_err(|e| format!("failed to run `git diff`: {e}"))?;
    if !diff_out.status.success() {
        let stderr = String::from_utf8_lossy(&diff_out.stderr);
        return Err(format!("`git diff --name-only {base}` failed: {stderr}"));
    }

    let untracked_out = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
        .map_err(|e| format!("failed to run `git ls-files`: {e}"))?;
    if !untracked_out.status.success() {
        let stderr = String::from_utf8_lossy(&untracked_out.stderr);
        return Err(format!("`git ls-files` failed: {stderr}"));
    }

    let mut files = Vec::new();
    for buf in [&diff_out.stdout, &untracked_out.stdout] {
        let s = String::from_utf8_lossy(buf);
        for line in s.lines().map(str::trim).filter(|l| !l.is_empty()) {
            files.push(line.to_string());
        }
    }
    Ok(files)
}

pub fn run_test(graph: &WorkspaceGraph, args: &[String]) -> i32 {
    let mut base = "HEAD".to_string();
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--base" => {
                i += 1;
                let Some(val) = args.get(i) else {
                    eprintln!("Error: `--base` requires a git revision");
                    return 2;
                };
                base = val.clone();
            }
            "--dry-run" => dry_run = true,
            other => {
                eprintln!("Unknown `xtask test` flag: {other}");
                return 2;
            }
        }
        i += 1;
    }

    let changed = match collect_changed_files(&graph.root, &base) {
        Ok(list) => list,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };
    let seeds = map_changed_paths(graph, &changed);
    let affected = reverse_closure(graph, &seeds);

    if affected.is_empty() {
        println!(
            "No workspace crates affected since `{base}` ({} changed file(s)).",
            changed.len()
        );
        return 0;
    }

    let list: Vec<&str> = affected.iter().map(String::as_str).collect();
    println!(
        "Affected crates (base={base}, {} changed file(s)): {}",
        changed.len(),
        list.join(", ")
    );

    let mut cargo_args = vec!["test".to_string()];
    if affected.len() == graph.packages.len() {
        cargo_args.push("--workspace".to_string());
    } else {
        for pkg in &affected {
            cargo_args.push("-p".to_string());
            cargo_args.push(pkg.clone());
        }
    }

    if dry_run {
        println!("Dry-run command: cargo {}", cargo_args.join(" "));
        return 0;
    }

    let _keep_dev_edge_type: DepKind = DepKind::Normal;
    match Command::new("cargo")
        .current_dir(&graph.root)
        .args(&cargo_args)
        .status()
    {
        Ok(status) if status.success() => 0,
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("Failed to execute `cargo test`: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{ArchConfig, DepEdge, PackageInfo};
    use std::path::PathBuf;

    fn sample_graph() -> WorkspaceGraph {
        let mut packages = BTreeMap::new();
        let mut add = |name: &str, dir: &str, deps: &[&str]| {
            packages.insert(
                name.to_string(),
                PackageInfo {
                    name: name.to_string(),
                    manifest_dir: dir.to_string(),
                    deps: deps
                        .iter()
                        .map(|d| DepEdge {
                            name: (*d).to_string(),
                            kind: DepKind::Normal,
                            external: false,
                        })
                        .collect(),
                },
            );
        };
        add("frame", "crates/frame", &[]);
        add("http-api", "crates/contracts/http-api", &["frame"]);
        add("render-api", "crates/contracts/render-api", &["frame"]);
        add(
            "proto-http",
            "crates/components/proto-http",
            &["frame", "http-api"],
        );
        add(
            "doc-render-plain",
            "crates/components/doc-render-plain",
            &["frame", "render-api"],
        );
        add(
            "d",
            "crates/assemblies/d",
            &["frame", "http-api", "proto-http"],
        );
        add(
            "d-minimal",
            "crates/assemblies/d-minimal",
            &["frame", "http-api", "proto-http", "doc-render-plain"],
        );

        WorkspaceGraph {
            root: PathBuf::from("."),
            arch: ArchConfig::default(),
            packages,
        }
    }

    #[test]
    fn reverse_closure_propagates_contract_change_to_dependents_only() {
        let g = sample_graph();
        let seeds = map_changed_paths(
            &g,
            &["crates/components/doc-render-plain/src/lib.rs".to_string()],
        );
        assert_eq!(seeds, BTreeSet::from(["doc-render-plain".to_string()]));

        let closure = reverse_closure(&g, &seeds);
        assert_eq!(
            closure,
            BTreeSet::from([
                "doc-render-plain".to_string(),
                "d-minimal".to_string(),
            ])
        );
    }

    #[test]
    fn prefix_matching_prefers_exact_dir_over_prefix_sibling() {
        let g = sample_graph();
        let seeds = map_changed_paths(
            &g,
            &["crates/assemblies/d-minimal/src/lib.rs".to_string()],
        );
        assert_eq!(seeds, BTreeSet::from(["d-minimal".to_string()]));
    }

    #[test]
    fn root_manifest_change_marks_all_packages() {
        let g = sample_graph();
        let seeds = map_changed_paths(&g, &["Cargo.toml".to_string()]);
        assert_eq!(seeds.len(), g.packages.len());
    }
}
