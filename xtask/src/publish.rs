//! Pure-`std` single-crate bundler and publisher for `d` (`cargo xtask publish-d`).
//!
//! Enforces `R-ARCH-9` (`publish_allowlist = ["d"]`) and packages the entire
//! multi-crate workspace into a single self-contained `d` crate with zero
//! internal crates.io dependencies.

use crate::check;
use crate::graph::WorkspaceGraph;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct CrateUnit {
    dir: &'static str,
    mod_name: &'static str,
    mcp_only: bool,
    deps: &'static [&'static str],
}

const BUNDLED_UNITS: &[CrateUnit] = &[
    CrateUnit {
        dir: "crates/frame",
        mod_name: "frame",
        mcp_only: false,
        deps: &[],
    },
    CrateUnit {
        dir: "crates/contracts/net-api",
        mod_name: "net_api",
        mcp_only: false,
        deps: &["frame"],
    },
    CrateUnit {
        dir: "crates/contracts/tls-api",
        mod_name: "tls_api",
        mcp_only: false,
        deps: &["frame", "net_api"],
    },
    CrateUnit {
        dir: "crates/contracts/http-api",
        mod_name: "http_api",
        mcp_only: false,
        deps: &["frame", "net_api"],
    },
    CrateUnit {
        dir: "crates/contracts/ws-api",
        mod_name: "ws_api",
        mcp_only: false,
        deps: &["frame", "http_api"],
    },
    CrateUnit {
        dir: "crates/contracts/proxy-api",
        mod_name: "proxy_api",
        mcp_only: false,
        deps: &["frame"],
    },
    CrateUnit {
        dir: "crates/contracts/files-api",
        mod_name: "files_api",
        mcp_only: false,
        deps: &["frame", "http_api"],
    },
    CrateUnit {
        dir: "crates/contracts/render-api",
        mod_name: "render_api",
        mcp_only: false,
        deps: &["frame"],
    },
    CrateUnit {
        dir: "crates/components/l4-transport",
        mod_name: "l4_transport",
        mcp_only: false,
        deps: &["frame", "net_api", "tls_api"],
    },
    CrateUnit {
        dir: "crates/components/l4-tls",
        mod_name: "l4_tls",
        mcp_only: false,
        deps: &["frame", "net_api", "tls_api"],
    },
    CrateUnit {
        dir: "crates/components/proto-http",
        mod_name: "proto_http",
        mcp_only: false,
        deps: &["frame", "net_api", "http_api"],
    },
    CrateUnit {
        dir: "crates/components/proto-ws",
        mod_name: "proto_ws",
        mcp_only: false,
        deps: &["frame", "net_api", "http_api", "ws_api"],
    },
    CrateUnit {
        dir: "crates/components/proto-tcp",
        mod_name: "proto_tcp",
        mcp_only: false,
        deps: &["frame", "net_api"],
    },
    CrateUnit {
        dir: "crates/components/ext-proxy",
        mod_name: "ext_proxy",
        mcp_only: false,
        deps: &["frame", "net_api", "tls_api", "http_api", "proxy_api"],
    },
    CrateUnit {
        dir: "crates/components/ext-observability",
        mod_name: "ext_observability",
        mcp_only: false,
        deps: &["frame", "net_api", "http_api", "proxy_api"],
    },
    CrateUnit {
        dir: "crates/components/ext-ratelimit",
        mod_name: "ext_ratelimit",
        mcp_only: false,
        deps: &["frame", "http_api"],
    },
    CrateUnit {
        dir: "crates/components/ext-auth",
        mod_name: "ext_auth",
        mcp_only: false,
        deps: &["frame", "http_api"],
    },
    CrateUnit {
        dir: "crates/components/ext-dev-kit",
        mod_name: "ext_dev_kit",
        mcp_only: false,
        deps: &["frame", "http_api", "ws_api"],
    },
    CrateUnit {
        dir: "crates/components/ext-compression",
        mod_name: "ext_compression",
        mcp_only: false,
        deps: &["frame", "http_api"],
    },
    CrateUnit {
        dir: "crates/components/ext-cors",
        mod_name: "ext_cors",
        mcp_only: false,
        deps: &["frame", "http_api"],
    },
    CrateUnit {
        dir: "crates/components/app-files",
        mod_name: "app_files",
        mcp_only: false,
        deps: &["frame", "http_api", "files_api"],
    },
    CrateUnit {
        dir: "crates/components/doc-render",
        mod_name: "doc_render",
        mcp_only: false,
        deps: &["frame", "render_api"],
    },
    CrateUnit {
        dir: "crates/components/viewer-markdown",
        mod_name: "viewer_markdown",
        mcp_only: false,
        deps: &["frame", "http_api", "files_api", "render_api"],
    },
    CrateUnit {
        dir: "crates/components/viewer-code",
        mod_name: "viewer_code",
        mcp_only: false,
        deps: &["frame", "http_api", "files_api", "render_api"],
    },
    CrateUnit {
        dir: "crates/components/viewer-media",
        mod_name: "viewer_media",
        mcp_only: false,
        deps: &["frame", "http_api", "files_api"],
    },
    CrateUnit {
        dir: "crates/entries/cli",
        mod_name: "d_cli",
        mcp_only: false,
        deps: &[],
    },
    CrateUnit {
        dir: "crates/entries/daemon",
        mod_name: "d_daemon",
        mcp_only: false,
        deps: &[
            "frame",
            "net_api",
            "http_api",
            "files_api",
            "render_api",
            "d_cli",
        ],
    },
    CrateUnit {
        dir: "crates/entries/mcp",
        mod_name: "d_mcp",
        mcp_only: true,
        deps: &["frame", "http_api", "d_cli", "d_daemon"],
    },
];

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn run_publish_d(graph: &WorkspaceGraph, args: &[String]) -> i32 {
    let mut execute = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" => execute = false,
            "--execute" => execute = true,
            other => {
                eprintln!("Unknown `xtask publish-d` flag: {other}");
                return 2;
            }
        }
    }

    let violations = check::check_architecture(graph);
    if !violations.is_empty() {
        eprintln!(
            "Refusing to publish: architecture check failed with {} violation(s):",
            violations.len()
        );
        for v in &violations {
            eprintln!("  - {v}");
        }
        return 1;
    }
    if graph.arch.publish_allowlist != ["d"] {
        eprintln!(
            "Refusing to publish: `publish_allowlist` must equal [\"d\"], found {:?}",
            graph.arch.publish_allowlist
        );
        return 1;
    }

    let version = match read_workspace_version(&graph.root) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("xtask publish-d error: {e}");
            return 1;
        }
    };

    let stage_dir = std::env::temp_dir()
        .join(format!("d-single-publish-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage_dir);
    let _guard = TempDirGuard(stage_dir.clone());

    if let Err(e) = stage_single_crate(&graph.root, &stage_dir, &version) {
        eprintln!("xtask publish-d staging failed: {e}");
        return 1;
    }

    if let Err(e) = verify_and_publish(&stage_dir, &version, execute) {
        eprintln!("xtask publish-d failed: {e}");
        return 1;
    }

    0
}

fn read_workspace_version(root: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("failed to read root Cargo.toml: {e}"))?;
    let mut in_ws_pkg = false;
    for line in raw.lines() {
        let s = line.trim();
        if s.starts_with('[') {
            in_ws_pkg = s == "[workspace.package]";
            continue;
        }
        if in_ws_pkg && let Some(rest) = s.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix('=') {
                let v = val.trim().trim_matches('"');
                if !v.is_empty() {
                    return Ok(v.to_string());
                }
            }
        }
    }
    Err("missing version in [workspace.package]".to_string())
}

fn stage_single_crate(
    root: &Path,
    stage: &Path,
    version: &str,
) -> Result<(), String> {
    let src_root = stage.join("src");
    let bundled_root = src_root.join("__d_crates");
    fs::create_dir_all(&bundled_root)
        .map_err(|e| format!("create staging dir: {e}"))?;

    for asset in ["LICENSE", "README.md"] {
        fs::copy(root.join(asset), stage.join(asset))
            .map_err(|e| format!("copy {asset}: {e}"))?;
    }

    let manifest = format!(
        r#"[package]
name = "d"
version = "{version}"
edition = "2024"
rust-version = "1.88"
authors = ["Liu Chong <mail@liuchong.io>"]
license-file = "LICENSE"
repository = "https://github.com/liuchong/d"
documentation = "https://docs.rs/d"
description = "D is a modular extensible network daemon with built-in HTTP and file services."
keywords = ["http", "server", "cli", "filesystem", "daemon"]
readme = "README.md"

[features]
default = ["mcp"]
mcp = []

[dependencies]
tokio = {{ version = "1", features = ["rt-multi-thread", "macros", "signal", "net", "io-util", "io-std", "sync", "time", "fs"] }}
rustls = {{ version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }}
tokio-rustls = {{ version = "0.26", default-features = false, features = ["ring", "tls12"] }}
rustls-pki-types = {{ version = "1", features = ["std"] }}
webpki-roots = "1"
flate2 = "1"
brotli = "8"
"#
    );
    fs::write(stage.join("Cargo.toml"), manifest)
        .map_err(|e| format!("write standalone Cargo.toml: {e}"))?;

    for unit in BUNDLED_UNITS {
        let src_dir = root.join(unit.dir).join("src");
        let dst_dir = bundled_root.join(unit.mod_name);
        fs::create_dir_all(&dst_dir)
            .map_err(|e| format!("mkdir {}: {e}", dst_dir.display()))?;

        let lib_raw = fs::read_to_string(src_dir.join("lib.rs"))
            .map_err(|e| format!("read {}/lib.rs: {e}", src_dir.display()))?;
        let own_mods = collect_direct_mods(&lib_raw);
        let dep_set: BTreeSet<&str> = unit.deps.iter().copied().collect();

        copy_and_rewrite_tree(
            &src_dir,
            &dst_dir,
            true,
            &format!("crate::__d_crates::{}", unit.mod_name),
            "crate::__d_crates",
            &dep_set,
            &own_mods,
        )?;
    }

    // Generate src/lib.rs from crates/assemblies/d/src/lib.rs
    let mut mod_decls = String::from("#[doc(hidden)]\npub mod __d_crates {\n");
    for unit in BUNDLED_UNITS {
        if unit.mcp_only {
            mod_decls.push_str("    #[cfg(feature = \"mcp\")]\n");
        }
        mod_decls.push_str(&format!("    pub mod {};\n", unit.mod_name));
    }
    mod_decls.push_str("}\n\n");

    let all_deps: BTreeSet<&str> =
        BUNDLED_UNITS.iter().map(|u| u.mod_name).collect();
    let d_lib_raw =
        fs::read_to_string(root.join("crates/assemblies/d/src/lib.rs"))
            .map_err(|e| format!("read d/src/lib.rs: {e}"))?;
    let mut d_lib_rewritten = rewrite_rust_paths(
        &d_lib_raw,
        None,
        "crate::__d_crates",
        &all_deps,
        &BTreeSet::new(),
    );
    d_lib_rewritten = d_lib_rewritten
        .replace(
            "pub use d_daemon::",
            "pub use crate::__d_crates::d_daemon::",
        )
        .replace("pub use frame;", "pub use crate::__d_crates::frame;")
        .replace("pub use http_api;", "pub use crate::__d_crates::http_api;");
    let mut inner_docs = String::new();
    let mut body = String::new();
    let mut in_header = true;
    for line in d_lib_rewritten.lines() {
        if in_header && (line.starts_with("//!") || line.trim().is_empty()) {
            inner_docs.push_str(line);
            inner_docs.push('\n');
        } else {
            in_header = false;
            body.push_str(line);
            body.push('\n');
        }
    }
    fs::write(
        src_root.join("lib.rs"),
        format!("{inner_docs}{mod_decls}{body}"),
    )
    .map_err(|e| format!("write staged src/lib.rs: {e}"))?;

    // Generate src/main.rs from crates/assemblies/d/src/main.rs
    let bin_deps = BTreeSet::from(["frame", "d_cli", "d_daemon", "d_mcp"]);
    let d_main_raw =
        fs::read_to_string(root.join("crates/assemblies/d/src/main.rs"))
            .map_err(|e| format!("read d/src/main.rs: {e}"))?;
    let d_main_rewritten = rewrite_rust_paths(
        &d_main_raw,
        None,
        "d::__d_crates",
        &bin_deps,
        &BTreeSet::new(),
    );
    fs::write(src_root.join("main.rs"), d_main_rewritten)
        .map_err(|e| format!("write staged src/main.rs: {e}"))?;

    Ok(())
}

fn copy_and_rewrite_tree(
    src_dir: &Path,
    dst_dir: &Path,
    is_crate_root: bool,
    self_prefix: &str,
    crates_prefix: &str,
    deps: &BTreeSet<&str>,
    own_mods: &BTreeSet<String>,
) -> Result<(), String> {
    fs::create_dir_all(dst_dir)
        .map_err(|e| format!("create {}: {e}", dst_dir.display()))?;
    for entry in fs::read_dir(src_dir)
        .map_err(|e| format!("read_dir {}: {e}", src_dir.display()))?
    {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if path.is_dir() {
            copy_and_rewrite_tree(
                &path,
                &dst_dir.join(&name),
                false,
                self_prefix,
                crates_prefix,
                deps,
                &BTreeSet::new(),
            )?;
        } else if name_str.ends_with(".rs") {
            let dst_name = if is_crate_root && name_str == "lib.rs" {
                "mod.rs"
            } else {
                name_str.as_ref()
            };
            let raw = fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let rewritten = rewrite_rust_paths(
                &raw,
                Some(self_prefix),
                crates_prefix,
                deps,
                own_mods,
            );
            fs::write(dst_dir.join(dst_name), rewritten)
                .map_err(|e| format!("write {dst_name}: {e}"))?;
        }
    }
    Ok(())
}

fn collect_direct_mods(lib_src: &str) -> BTreeSet<String> {
    let mut mods = BTreeSet::new();
    for line in lib_src.lines() {
        let s = line.trim();
        let rest = s
            .strip_prefix("pub mod ")
            .or_else(|| s.strip_prefix("pub(crate) mod "))
            .or_else(|| s.strip_prefix("mod "));
        if let Some(after) = rest {
            let ident: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() {
                mods.insert(ident);
            }
        }
    }
    mods
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn prev_sig_char(b: &[u8], mut i: usize) -> Option<u8> {
    while i > 0 {
        i -= 1;
        if !b[i].is_ascii_whitespace() {
            return Some(b[i]);
        }
    }
    None
}

fn prev_ends_with_double_colon(b: &[u8], mut i: usize) -> bool {
    while i > 0 && b[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    i >= 2 && b[i - 1] == b':' && b[i - 2] == b':'
}

fn prev_before_double_colon_blocks_root(b: &[u8], i: usize) -> bool {
    // `i` points right after `::`, step before `::`
    if i < 2 {
        return false;
    }
    let Some(c) = prev_sig_char(b, i - 2) else {
        return false;
    };
    if matches!(c, b'>' | b']' | b')') {
        return true;
    }
    if !is_ident_cont(c) {
        return false;
    }
    // Find end index of previous significant char
    let mut end = i - 2;
    while end > 0 && b[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_ident_cont(b[start - 1]) {
        start -= 1;
    }
    let word = std::str::from_utf8(&b[start..end]).unwrap_or("");
    !matches!(
        word,
        "use"
            | "in"
            | "return"
            | "match"
            | "if"
            | "else"
            | "while"
            | "for"
            | "loop"
            | "let"
            | "const"
            | "static"
            | "type"
            | "as"
            | "where"
            | "mut"
            | "ref"
            | "await"
            | "yield"
    )
}

fn rewrite_rust_paths(
    src: &str,
    self_prefix: Option<&str>,
    crates_prefix: &str,
    deps: &BTreeSet<&str>,
    own_mods: &BTreeSet<String>,
) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 256);
    let mut i = 0;

    while i < b.len() {
        // Line comment
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            let start = i;
            i += 2;
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            out.push_str(&src[start..i]);
            continue;
        }
        // Nested block comment
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            let start = i;
            let mut depth = 1usize;
            i += 2;
            while i < b.len() && depth > 0 {
                if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if i + 1 < b.len() && b[i] == b'*' && b[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            out.push_str(&src[start..i]);
            continue;
        }
        // Raw strings r#"..."#, br#"..."#, cr#"..."#
        if let Some(raw_end) = try_scan_raw_string(b, i) {
            out.push_str(&src[i..raw_end]);
            i = raw_end;
            continue;
        }
        // Byte/C string literals b"...", c"..."
        if i + 1 < b.len() && (b[i] == b'b' || b[i] == b'c') && b[i + 1] == b'"'
        {
            let start = i;
            i = scan_quoted(b, i + 2, b'"');
            out.push_str(&src[start..i]);
            continue;
        }
        // Byte char literal b'...'
        if i + 2 < b.len()
            && b[i] == b'b'
            && b[i + 1] == b'\''
            && let Some(end) = try_scan_char_lit(b, i + 1)
        {
            out.push_str(&src[i..end]);
            i = end;
            continue;
        }
        // Normal string "..."
        if b[i] == b'"' {
            let start = i;
            i = scan_quoted(b, i + 1, b'"');
            out.push_str(&src[start..i]);
            continue;
        }
        // Char literal vs lifetime
        if b[i] == b'\'' {
            if let Some(end) = try_scan_char_lit(b, i) {
                out.push_str(&src[i..end]);
                i = end;
                continue;
            }
            out.push('\'');
            i += 1;
            continue;
        }
        // Leading `::dep::`
        if i + 1 < b.len()
            && b[i] == b':'
            && b[i + 1] == b':'
            && !prev_before_double_colon_blocks_root(b, i + 2)
        {
            let j = skip_ws(b, i + 2);
            if j < b.len() && is_ident_start(b[j]) {
                let mut k = j + 1;
                while k < b.len() && is_ident_cont(b[k]) {
                    k += 1;
                }
                let ident = &src[j..k];
                let after = skip_ws(b, k);
                if deps.contains(ident)
                    && after + 1 < b.len()
                    && b[after] == b':'
                    && b[after + 1] == b':'
                {
                    out.push_str(crates_prefix);
                    out.push_str("::");
                    out.push_str(ident);
                    i = k;
                    continue;
                }
            }
        }
        // Identifier
        if is_ident_start(b[i]) {
            let start = i;
            i += 1;
            while i < b.len() && is_ident_cont(b[i]) {
                i += 1;
            }
            let ident = &src[start..i];
            let after = skip_ws(b, i);
            let followed_by_dc =
                after + 1 < b.len() && b[after] == b':' && b[after + 1] == b':';
            let preceded_by_dot_or_dc = prev_sig_char(b, start) == Some(b'.')
                || prev_ends_with_double_colon(b, start);

            if followed_by_dc && !preceded_by_dot_or_dc {
                if ident == "crate" {
                    if let Some(sp) = self_prefix {
                        out.push_str(sp);
                        continue;
                    }
                } else if deps.contains(ident) && !own_mods.contains(ident) {
                    out.push_str(crates_prefix);
                    out.push_str("::");
                    out.push_str(ident);
                    continue;
                }
            }
            out.push_str(ident);
            continue;
        }

        // Advance one UTF-8 char safely
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

fn scan_quoted(b: &[u8], mut i: usize, quote: u8) -> usize {
    while i < b.len() {
        if b[i] == b'\\' {
            i = (i + 2).min(b.len());
        } else if b[i] == quote {
            return i + 1;
        } else {
            i += 1;
        }
    }
    b.len()
}

fn try_scan_raw_string(b: &[u8], i: usize) -> Option<usize> {
    let mut p = i;
    if p < b.len() && (b[p] == b'b' || b[p] == b'c') {
        p += 1;
    }
    if p >= b.len() || b[p] != b'r' {
        return None;
    }
    p += 1;
    let mut hashes = 0usize;
    while p < b.len() && b[p] == b'#' {
        hashes += 1;
        p += 1;
    }
    if p >= b.len() || b[p] != b'"' {
        return None;
    }
    p += 1;
    while p < b.len() {
        if b[p] == b'"' {
            let mut match_h = 0usize;
            while match_h < hashes
                && p + 1 + match_h < b.len()
                && b[p + 1 + match_h] == b'#'
            {
                match_h += 1;
            }
            if match_h == hashes {
                return Some(p + 1 + hashes);
            }
        }
        p += 1;
    }
    Some(b.len())
}

fn try_scan_char_lit(b: &[u8], i: usize) -> Option<usize> {
    if i + 1 >= b.len() || b[i] != b'\'' {
        return None;
    }
    if b[i + 1] == b'\\' {
        let mut p = i + 2;
        while p < b.len() && p < i + 12 {
            if b[p] == b'\'' {
                return Some(p + 1);
            }
            p += 1;
        }
        return None;
    }
    // Single UTF-8 char followed immediately by `'`
    let rest = std::str::from_utf8(&b[i + 1..]).ok()?;
    let ch = rest.chars().next()?;
    if ch == '\'' || ch == '\n' {
        return None;
    }
    let next_idx = i + 1 + ch.len_utf8();
    if next_idx < b.len() && b[next_idx] == b'\'' {
        Some(next_idx + 1)
    } else {
        None
    }
}

fn verify_and_publish(
    stage: &Path,
    expected_version: &str,
    execute: bool,
) -> Result<(), String> {
    println!(
        "Staging single-crate `d` v{expected_version} at {}",
        stage.display()
    );

    let run = |args: &[&str]| -> Result<(), String> {
        let status = Command::new("cargo")
            .current_dir(stage)
            .args(args)
            .status()
            .map_err(|e| {
                format!("failed to run `cargo {}`: {e}", args.join(" "))
            })?;
        if !status.success() {
            return Err(format!(
                "`cargo {}` failed with {status}",
                args.join(" ")
            ));
        }
        Ok(())
    };

    run(&["check", "--no-default-features"])?;
    run(&["build"])?;

    let bin_out = Command::new(stage.join("target/debug/d"))
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to run staged binary: {e}"))?;
    if !bin_out.status.success() {
        return Err("staged binary `--version` failed".to_string());
    }
    let ver_stdout = String::from_utf8_lossy(&bin_out.stdout);
    let expected = format!("d {expected_version}");
    if ver_stdout.trim() != expected {
        return Err(format!(
            "staged binary version mismatch: expected `{expected}`, got `{}`",
            ver_stdout.trim()
        ));
    }
    println!("Verified staged single-crate binary: {}", ver_stdout.trim());

    if execute {
        run(&["publish", "--allow-dirty"])?;
        println!(
            "Published single-crate `d` v{expected_version} to crates.io!"
        );
    } else {
        run(&["publish", "--dry-run", "--allow-dirty"])?;
        println!(
            "Dry-run packaging & verification succeeded for single-crate `d` v{expected_version}."
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_crate_deps_and_leading_colon_but_preserves_strings_lifetimes_and_own_mods()
     {
        let deps = BTreeSet::from(["frame", "http_api"]);
        let own = BTreeSet::from(["frame".to_string()]);
        let src = r##"
use ::frame::{Context, BoxFuture};
use http_api::HttpRequest;
use crate::frame::decode_frame;
pub mod frame;
fn f<'a>(s: &'a str) -> &'static str {
    let lit = "frame::keep crate::keep";
    let rlit = r#"http_api::keep"#;
    let c = '"';
    let _ = crate::handshake::ok();
    s
}
"##;
        let out = rewrite_rust_paths(
            src,
            Some("crate::__d_crates::proto_ws"),
            "crate::__d_crates",
            &deps,
            &own,
        );
        assert!(
            out.contains("use crate::__d_crates::frame::{Context, BoxFuture};")
        );
        assert!(out.contains("use crate::__d_crates::http_api::HttpRequest;"));
        assert!(
            out.contains(
                "use crate::__d_crates::proto_ws::frame::decode_frame;"
            )
        );
        assert!(out.contains("let lit = \"frame::keep crate::keep\";"));
        assert!(out.contains("let rlit = r#\"http_api::keep\"#;"));
        assert!(
            out.contains(
                "let _ = crate::__d_crates::proto_ws::handshake::ok();"
            )
        );
    }
}
