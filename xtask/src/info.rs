//! Render workspace layer inventory, assembly matrix, and contract map.

use crate::check::{
    build_layer_map, component_implementations_of, forward_closure,
};
use crate::graph::{DepKind, Layer, WorkspaceGraph};
use std::collections::BTreeSet;
use std::fmt::Write as _;

pub fn format_info(graph: &WorkspaceGraph) -> String {
    let mut dummy = Vec::new();
    let layer_map = build_layer_map(graph, &mut dummy);

    let mut out = String::new();
    let _ = writeln!(out, "=== Workspace Architecture Summary ===");
    let _ = writeln!(out, "Total workspace packages: {}", graph.packages.len());
    for layer in Layer::ALL {
        let members = graph.arch.members_for(layer);
        let _ = writeln!(
            out,
            "  {:<11} ({:>2}): {}",
            layer.label(),
            members.len(),
            members.join(", ")
        );
    }

    let _ = writeln!(out, "\n=== Components x Assemblies ===");
    let assemblies = &graph.arch.assemblies;
    let closures: Vec<(String, BTreeSet<String>)> = assemblies
        .iter()
        .map(|a| (a.clone(), forward_closure(graph, a)))
        .collect();

    let _ = write!(out, "{:<20}", "Component");
    for a in assemblies {
        let _ = write!(out, " | {:<10}", a);
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", "-".repeat(20 + assemblies.len() * 13));

    for comp in &graph.arch.components {
        let _ = write!(out, "{:<20}", comp);
        for (_, set) in &closures {
            let mark = if set.contains(comp) { "[x]" } else { "[ ]" };
            let _ = write!(out, " | {:<10}", mark);
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "\n=== Contracts -> Component Implementations ===");
    for contract in &graph.arch.contracts {
        let impls = component_implementations_of(graph, &layer_map, contract);
        let _ = writeln!(
            out,
            "  {:<14} -> {}",
            contract,
            if impls.is_empty() {
                "(none)".to_string()
            } else {
                impls.join(", ")
            }
        );
    }

    let _ = writeln!(out, "\n=== Zero-External-Deps Audit ===");
    let mut dirty = Vec::new();
    for name in &graph.arch.zero_external_deps {
        if let Some(pkg) = graph.packages.get(name) {
            let exts: Vec<&str> = pkg
                .deps
                .iter()
                .filter(|d| d.kind != DepKind::Dev && d.external)
                .map(|d| d.name.as_str())
                .collect();
            if !exts.is_empty() {
                dirty.push(format!("{name}: {}", exts.join(", ")));
            }
        }
    }
    let _ = writeln!(
        out,
        "  Declared pure packages: {}",
        graph.arch.zero_external_deps.len()
    );
    if dirty.is_empty() {
        let _ = writeln!(out, "  External leak status:   OK (0 violations)");
    } else {
        let _ = writeln!(
            out,
            "  External leak status:   FAILED ({})",
            dirty.join("; ")
        );
    }

    out
}

pub fn run_info(graph: &WorkspaceGraph) -> i32 {
    print!("{}", format_info(graph));
    0
}
