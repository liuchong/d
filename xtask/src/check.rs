//! Enforce the 8 architecture invariants (`R-ARCH-1` .. `R-ARCH-8`).

use crate::graph::{ArchConfig, DepKind, Layer, WorkspaceGraph};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub rule: &'static str,
    pub package: String,
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.rule, self.package, self.message)
    }
}

pub fn check_architecture(graph: &WorkspaceGraph) -> Vec<Violation> {
    let mut violations = Vec::new();
    let layer_map = build_layer_map(graph, &mut violations);

    check_zero_external_deps(graph, &mut violations);
    check_layer_boundaries(graph, &layer_map, &mut violations);
    check_minimal_exclusion(graph, &mut violations);
    check_acyclic_and_multi_impl(graph, &layer_map, &mut violations);
    check_skills_integrity(&graph.root, &mut violations);

    violations
}

/// Verify bundled repository Agent Skills (`skills/d` and `skills/d-dev`) when present.
pub(crate) fn check_skills_integrity(
    root: &std::path::Path,
    violations: &mut Vec<Violation>,
) {
    let skills_root = root.join("skills");
    if !skills_root.is_dir() {
        return;
    }

    for skill in ["d", "d-dev"] {
        let skill_dir = skills_root.join(skill);
        let entry = skill_dir.join("SKILL.md");
        if !entry.is_file() {
            violations.push(Violation {
                rule: "R-ARCH-8",
                package: format!("skills/{skill}"),
                message: "missing SKILL.md entrypoint".to_string(),
            });
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&entry) else {
            violations.push(Violation {
                rule: "R-ARCH-8",
                package: format!("skills/{skill}"),
                message: "failed to read SKILL.md as UTF-8".to_string(),
            });
            continue;
        };
        let mut lines = body.lines();
        let valid_frontmatter = if lines.next() == Some("---") {
            let mut has_name = false;
            let mut has_desc = false;
            let mut closed = false;
            for line in lines {
                if line == "---" {
                    closed = true;
                    break;
                }
                if line.starts_with("name:")
                    && !line["name:".len()..].trim().is_empty()
                {
                    has_name = true;
                }
                if line.starts_with("description:")
                    && !line["description:".len()..].trim().is_empty()
                {
                    has_desc = true;
                }
            }
            closed && has_name && has_desc
        } else {
            false
        };
        if !valid_frontmatter {
            violations.push(Violation {
                rule: "R-ARCH-8",
                package: format!("skills/{skill}"),
                message: "SKILL.md requires YAML frontmatter with `name` and `description`".to_string(),
            });
        }

        let mut rest = body.as_str();
        while let Some(idx) = rest.find("](references/") {
            let after = &rest[idx + 2..];
            if let Some(end) = after.find(')') {
                let rel = &after[..end];
                if !skill_dir.join(rel).is_file() {
                    violations.push(Violation {
                        rule: "R-ARCH-8",
                        package: format!("skills/{skill}"),
                        message: format!(
                            "broken local skill reference `{rel}` in SKILL.md"
                        ),
                    });
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
}

pub fn run_check(graph: &WorkspaceGraph) -> i32 {
    let violations = check_architecture(graph);
    if violations.is_empty() {
        println!(
            "Architecture check passed: {} workspace packages verified across all 8 rules.",
            graph.packages.len()
        );
        0
    } else {
        eprintln!(
            "Architecture check FAILED with {} violation(s):",
            violations.len()
        );
        for v in &violations {
            eprintln!("  - {v}");
        }
        1
    }
}

/// Build `pkg_name -> Layer` map while enforcing `R-ARCH-1`.
pub(crate) fn build_layer_map(
    graph: &WorkspaceGraph,
    violations: &mut Vec<Violation>,
) -> BTreeMap<String, Layer> {
    let mut map = BTreeMap::new();
    for layer in Layer::ALL {
        for pkg_name in graph.arch.members_for(layer) {
            if !graph.packages.contains_key(pkg_name) {
                violations.push(Violation {
                    rule: "R-ARCH-1",
                    package: pkg_name.clone(),
                    message: format!(
                        "listed in architecture.{} but not found in workspace members",
                        layer.label()
                    ),
                });
                continue;
            }
            if let Some(prev) = map.insert(pkg_name.clone(), layer) {
                violations.push(Violation {
                    rule: "R-ARCH-1",
                    package: pkg_name.clone(),
                    message: format!(
                        "assigned to multiple layers: `{}` and `{}`",
                        prev.label(),
                        layer.label()
                    ),
                });
            }
        }
    }

    for name in graph.packages.keys() {
        if !map.contains_key(name) {
            violations.push(Violation {
                rule: "R-ARCH-1",
                package: name.clone(),
                message: "workspace package is missing from [workspace.metadata.architecture]"
                    .to_string(),
            });
        }
    }

    map
}

/// Enforce `R-ARCH-2`: packages in `zero_external_deps` cannot depend on external crates.
fn check_zero_external_deps(
    graph: &WorkspaceGraph,
    violations: &mut Vec<Violation>,
) {
    for pkg_name in &graph.arch.zero_external_deps {
        let Some(pkg) = graph.packages.get(pkg_name) else {
            violations.push(Violation {
                rule: "R-ARCH-2",
                package: pkg_name.clone(),
                message:
                    "listed in `zero_external_deps` but missing from workspace"
                        .to_string(),
            });
            continue;
        };
        for dep in &pkg.deps {
            if dep.kind != DepKind::Dev && dep.external {
                violations.push(Violation {
                    rule: "R-ARCH-2",
                    package: pkg_name.clone(),
                    message: format!(
                        "declared in `zero_external_deps` but depends on external crate `{}`",
                        dep.name
                    ),
                });
            }
        }
    }
}

/// Enforce `R-ARCH-3` .. `R-ARCH-7` dependency direction rules.
fn check_layer_boundaries(
    graph: &WorkspaceGraph,
    layer_map: &BTreeMap<String, Layer>,
    violations: &mut Vec<Violation>,
) {
    for (name, pkg) in &graph.packages {
        let Some(&src_layer) = layer_map.get(name) else {
            continue;
        };
        for dep in &pkg.deps {
            if dep.kind == DepKind::Dev {
                continue;
            }
            match src_layer {
                Layer::Kernel => {
                    violations.push(Violation {
                        rule: "R-ARCH-3",
                        package: name.clone(),
                        message: format!(
                            "kernel crate must have zero normal/build dependencies, found `{}`",
                            dep.name
                        ),
                    });
                }
                Layer::Contracts => {
                    if dep.external {
                        violations.push(Violation {
                            rule: "R-ARCH-4",
                            package: name.clone(),
                            message: format!(
                                "contract crate cannot depend on external crate `{}`",
                                dep.name
                            ),
                        });
                    } else if let Some(&dst_layer) = layer_map.get(&dep.name)
                        && !matches!(
                            dst_layer,
                            Layer::Kernel | Layer::Contracts
                        )
                    {
                        violations.push(Violation {
                            rule: "R-ARCH-4",
                            package: name.clone(),
                            message: format!(
                                "contract crate may only depend on kernel/contracts, found {} `{}`",
                                dst_layer.label(),
                                dep.name
                            ),
                        });
                    }
                }
                Layer::Components => {
                    if !dep.external
                        && let Some(&dst_layer) = layer_map.get(&dep.name)
                        && !matches!(
                            dst_layer,
                            Layer::Kernel | Layer::Contracts
                        )
                    {
                        violations.push(Violation {
                            rule: "R-ARCH-5",
                            package: name.clone(),
                            message: format!(
                                "component crate may only depend on kernel/contracts inside workspace, found {} `{}`",
                                dst_layer.label(),
                                dep.name
                            ),
                        });
                    }
                }
                Layer::Entries => {
                    if !dep.external
                        && let Some(&dst_layer) = layer_map.get(&dep.name)
                        && matches!(
                            dst_layer,
                            Layer::Components
                                | Layer::Assemblies
                                | Layer::Tools
                        )
                    {
                        violations.push(Violation {
                            rule: "R-ARCH-6",
                            package: name.clone(),
                            message: format!(
                                "entry crate must not depend on {} `{}`",
                                dst_layer.label(),
                                dep.name
                            ),
                        });
                    }
                }
                Layer::Assemblies => {
                    if !dep.external
                        && let Some(&dst_layer) = layer_map.get(&dep.name)
                        && matches!(dst_layer, Layer::Assemblies | Layer::Tools)
                    {
                        violations.push(Violation {
                            rule: "R-ARCH-7",
                            package: name.clone(),
                            message: format!(
                                "assembly crate must not depend on {} `{}`",
                                dst_layer.label(),
                                dep.name
                            ),
                        });
                    }
                }
                Layer::Tools => {}
            }
        }
    }
}

/// Enforce the `R-ARCH-7` exclusion invariant on `minimal_assembly`.
fn check_minimal_exclusion(
    graph: &WorkspaceGraph,
    violations: &mut Vec<Violation>,
) {
    let Some(minimal_name) = &graph.arch.minimal_assembly else {
        return;
    };
    if !graph.packages.contains_key(minimal_name) {
        violations.push(Violation {
            rule: "R-ARCH-7",
            package: minimal_name.clone(),
            message: "`minimal_assembly` does not exist in workspace"
                .to_string(),
        });
        return;
    }

    let forbidden: BTreeSet<&str> = graph
        .arch
        .minimal_forbidden
        .iter()
        .map(String::as_str)
        .collect();
    let closure = forward_closure(graph, minimal_name);
    for reachable_dep in closure {
        if forbidden.contains(reachable_dep.as_str()) {
            violations.push(Violation {
                rule: "R-ARCH-7",
                package: minimal_name.clone(),
                message: format!(
                    "minimal assembly `{minimal_name}` transitively includes forbidden dependency `{reachable_dep}`"
                ),
            });
        }
    }
}

/// Collect all direct and transitive non-dev dependency names of `root_pkg`.
pub(crate) fn forward_closure(
    graph: &WorkspaceGraph,
    root_pkg: &str,
) -> BTreeSet<String> {
    let mut visited_ws = BTreeSet::new();
    let mut reached_all = BTreeSet::new();
    let mut stack = vec![root_pkg.to_string()];

    while let Some(cur) = stack.pop() {
        if !visited_ws.insert(cur.clone()) {
            continue;
        }
        let Some(pkg) = graph.packages.get(&cur) else {
            continue;
        };
        for dep in &pkg.deps {
            if dep.kind == DepKind::Dev {
                continue;
            }
            reached_all.insert(dep.name.clone());
            if !dep.external {
                stack.push(dep.name.clone());
            }
        }
    }
    reached_all
}

/// Enforce `R-ARCH-8`: acyclic workspace DAG & multi-implementation contracts.
fn check_acyclic_and_multi_impl(
    graph: &WorkspaceGraph,
    layer_map: &BTreeMap<String, Layer>,
    violations: &mut Vec<Violation>,
) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        Unvisited,
        Visiting,
        Done,
    }

    fn dfs(
        node: &str,
        graph: &WorkspaceGraph,
        marks: &mut BTreeMap<String, Mark>,
        trail: &mut Vec<String>,
        violations: &mut Vec<Violation>,
    ) {
        marks.insert(node.to_string(), Mark::Visiting);
        trail.push(node.to_string());
        if let Some(pkg) = graph.packages.get(node) {
            for dep in &pkg.deps {
                if dep.kind == DepKind::Dev || dep.external {
                    continue;
                }
                match marks.get(&dep.name).copied().unwrap_or(Mark::Unvisited) {
                    Mark::Unvisited => {
                        dfs(&dep.name, graph, marks, trail, violations);
                    }
                    Mark::Visiting => {
                        violations.push(Violation {
                            rule: "R-ARCH-8",
                            package: node.to_string(),
                            message: format!(
                                "workspace dependency cycle detected involving `{}` -> `{}`",
                                trail.join(" -> "),
                                dep.name
                            ),
                        });
                    }
                    Mark::Done => {}
                }
            }
        }
        trail.pop();
        marks.insert(node.to_string(), Mark::Done);
    }

    let mut marks = BTreeMap::new();
    let mut trail = Vec::new();
    for name in graph.packages.keys() {
        if marks.get(name).copied().unwrap_or(Mark::Unvisited)
            == Mark::Unvisited
        {
            dfs(name, graph, &mut marks, &mut trail, violations);
        }
    }

    // Multi-implementation contract invariant.
    for contract in &graph.arch.multi_impl_contracts {
        let providers =
            component_implementations_of(graph, layer_map, contract);
        if providers.len() < 2 {
            violations.push(Violation {
                rule: "R-ARCH-8",
                package: contract.clone(),
                message: format!(
                    "expected >= 2 distinct component implementations for `{contract}`, found {}: {:?}",
                    providers.len(),
                    providers
                ),
            });
        }
    }
}

pub(crate) fn component_implementations_of(
    graph: &WorkspaceGraph,
    layer_map: &BTreeMap<String, Layer>,
    contract: &str,
) -> Vec<String> {
    let mut impls = Vec::new();
    for (name, pkg) in &graph.packages {
        if layer_map.get(name) != Some(&Layer::Components) {
            continue;
        }
        let implements = pkg.deps.iter().any(|d| {
            d.kind != DepKind::Dev && !d.external && d.name == contract
        });
        if implements {
            impls.push(name.clone());
        }
    }
    impls
}

#[allow(dead_code)]
fn _keep_arch_referenced(_: &ArchConfig) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{ArchConfig, DepEdge, PackageInfo};

    fn ws_dep(name: &str) -> DepEdge {
        DepEdge {
            name: name.to_string(),
            kind: DepKind::Normal,
            external: false,
        }
    }

    fn ext_dep(name: &str) -> DepEdge {
        DepEdge {
            name: name.to_string(),
            kind: DepKind::Normal,
            external: true,
        }
    }

    fn valid_synthetic_graph() -> WorkspaceGraph {
        let mut packages = BTreeMap::new();
        let mut add = |name: &str, dir: &str, deps: Vec<DepEdge>| {
            packages.insert(
                name.to_string(),
                PackageInfo {
                    name: name.to_string(),
                    manifest_dir: dir.to_string(),
                    deps,
                },
            );
        };

        add("d-frame", "crates/frame", vec![]);
        add(
            "d-net-api",
            "crates/contracts/net-api",
            vec![ws_dep("d-frame")],
        );
        add(
            "d-render-api",
            "crates/contracts/render-api",
            vec![ws_dep("d-frame")],
        );
        add(
            "d-l4-transport",
            "crates/components/l4-transport",
            vec![ws_dep("d-frame"), ws_dep("d-net-api"), ext_dep("tokio")],
        );
        add(
            "d-doc-render",
            "crates/components/doc-render",
            vec![ws_dep("d-frame"), ws_dep("d-render-api")],
        );
        add(
            "d-doc-render-plain",
            "crates/components/doc-render-plain",
            vec![ws_dep("d-frame"), ws_dep("d-render-api")],
        );
        add(
            "d-daemon",
            "crates/entries/daemon",
            vec![ws_dep("d-frame"), ws_dep("d-net-api"), ext_dep("tokio")],
        );
        add(
            "d",
            "crates/assemblies/d",
            vec![
                ws_dep("d-frame"),
                ws_dep("d-daemon"),
                ws_dep("d-l4-transport"),
                ws_dep("d-doc-render"),
            ],
        );
        add(
            "d-minimal",
            "crates/assemblies/d-minimal",
            vec![
                ws_dep("d-frame"),
                ws_dep("d-daemon"),
                ws_dep("d-l4-transport"),
                ws_dep("d-doc-render-plain"),
            ],
        );
        add("xtask", "xtask", vec![]);

        WorkspaceGraph {
            root: std::path::PathBuf::from("."),
            arch: ArchConfig {
                kernel: vec!["d-frame".into()],
                contracts: vec!["d-net-api".into(), "d-render-api".into()],
                components: vec![
                    "d-l4-transport".into(),
                    "d-doc-render".into(),
                    "d-doc-render-plain".into(),
                ],
                entries: vec!["d-daemon".into()],
                assemblies: vec!["d".into(), "d-minimal".into()],
                tools: vec!["xtask".into()],
                zero_external_deps: vec![
                    "d-frame".into(),
                    "d-net-api".into(),
                    "d-render-api".into(),
                    "d-doc-render".into(),
                    "d-doc-render-plain".into(),
                    "xtask".into(),
                ],
                minimal_assembly: Some("d-minimal".into()),
                minimal_forbidden: vec!["d-doc-render".into(), "brotli".into()],
                multi_impl_contracts: vec!["d-render-api".into()],
            },
            packages,
        }
    }

    #[test]
    fn valid_graph_passes_all_8_rules() {
        let g = valid_synthetic_graph();
        let v = check_architecture(&g);
        assert!(v.is_empty(), "unexpected violations: {v:?}");
    }

    #[test]
    fn r_arch_1_rejects_unclassified_package() {
        let mut g = valid_synthetic_graph();
        g.packages.insert(
            "rogue".into(),
            PackageInfo {
                name: "rogue".into(),
                manifest_dir: "crates/rogue".into(),
                deps: vec![],
            },
        );
        let v = check_architecture(&g);
        assert!(
            v.iter()
                .any(|x| x.rule == "R-ARCH-1" && x.package == "rogue")
        );
    }

    #[test]
    fn r_arch_2_rejects_external_dep_in_zero_external_deps_crate() {
        let mut g = valid_synthetic_graph();
        g.packages
            .get_mut("d-doc-render")
            .unwrap()
            .deps
            .push(ext_dep("mock-external"));
        let v = check_architecture(&g);
        assert!(
            v.iter()
                .any(|x| x.rule == "R-ARCH-2" && x.package == "d-doc-render")
        );
    }

    #[test]
    fn r_arch_3_rejects_kernel_dependency() {
        let mut g = valid_synthetic_graph();
        g.packages
            .get_mut("d-frame")
            .unwrap()
            .deps
            .push(ext_dep("libc"));
        let v = check_architecture(&g);
        assert!(
            v.iter()
                .any(|x| x.rule == "R-ARCH-3" && x.package == "d-frame")
        );
    }

    #[test]
    fn r_arch_4_rejects_contract_depending_on_component() {
        let mut g = valid_synthetic_graph();
        g.packages
            .get_mut("d-net-api")
            .unwrap()
            .deps
            .push(ws_dep("d-l4-transport"));
        let v = check_architecture(&g);
        assert!(
            v.iter()
                .any(|x| x.rule == "R-ARCH-4" && x.package == "d-net-api")
        );
    }

    #[test]
    fn r_arch_5_rejects_component_to_component_dependency() {
        let mut g = valid_synthetic_graph();
        g.packages
            .get_mut("d-l4-transport")
            .unwrap()
            .deps
            .push(ws_dep("d-doc-render"));
        let v = check_architecture(&g);
        assert!(
            v.iter()
                .any(|x| x.rule == "R-ARCH-5" && x.package == "d-l4-transport")
        );
    }

    #[test]
    fn r_arch_6_rejects_entry_depending_on_component() {
        let mut g = valid_synthetic_graph();
        g.packages
            .get_mut("d-daemon")
            .unwrap()
            .deps
            .push(ws_dep("d-l4-transport"));
        let v = check_architecture(&g);
        assert!(
            v.iter()
                .any(|x| x.rule == "R-ARCH-6" && x.package == "d-daemon")
        );
    }

    #[test]
    fn r_arch_7_rejects_assembly_to_assembly_and_minimal_forbidden() {
        let mut g = valid_synthetic_graph();
        g.packages
            .get_mut("d-minimal")
            .unwrap()
            .deps
            .push(ws_dep("d-doc-render"));
        g.packages
            .get_mut("d-minimal")
            .unwrap()
            .deps
            .push(ws_dep("d"));
        let v = check_architecture(&g);
        let r7: Vec<_> = v.iter().filter(|x| x.rule == "R-ARCH-7").collect();
        assert!(
            r7.len() >= 2,
            "expected both R-ARCH-7 checks to fire: {r7:?}"
        );
    }

    #[test]
    fn r_arch_7_rejects_gateway_crates_and_rustls_in_minimal() {
        let mut g = valid_synthetic_graph();
        g.arch
            .components
            .extend(["d-l4-tls".into(), "d-ext-proxy".into()]);
        g.arch.minimal_forbidden.extend([
            "d-l4-tls".into(),
            "d-ext-proxy".into(),
            "rustls".into(),
        ]);
        g.packages.insert(
            "d-l4-tls".into(),
            PackageInfo {
                name: "d-l4-tls".into(),
                manifest_dir: "crates/components/l4-tls".into(),
                deps: vec![ext_dep("rustls")],
            },
        );
        g.packages.insert(
            "d-ext-proxy".into(),
            PackageInfo {
                name: "d-ext-proxy".into(),
                manifest_dir: "crates/components/ext-proxy".into(),
                deps: vec![ws_dep("d-net-api")],
            },
        );
        let dm = g.packages.get_mut("d-minimal").unwrap();
        dm.deps.push(ws_dep("d-l4-tls"));
        dm.deps.push(ws_dep("d-ext-proxy"));

        let v = check_architecture(&g);
        let r7: Vec<_> = v.iter().filter(|x| x.rule == "R-ARCH-7").collect();
        assert!(
            r7.iter().any(|x| x.message.contains("d-l4-tls"))
                && r7.iter().any(|x| x.message.contains("d-ext-proxy"))
                && r7.iter().any(|x| x.message.contains("rustls")),
            "expected R-ARCH-7 violations for d-l4-tls, d-ext-proxy, and rustls: {r7:?}"
        );
    }

    #[test]
    fn r_arch_8_rejects_cycle_and_single_impl_contract() {
        let mut g = valid_synthetic_graph();
        g.packages
            .get_mut("d-net-api")
            .unwrap()
            .deps
            .push(ws_dep("d-render-api"));
        g.packages
            .get_mut("d-render-api")
            .unwrap()
            .deps
            .push(ws_dep("d-net-api"));
        g.packages
            .get_mut("d-doc-render-plain")
            .unwrap()
            .deps
            .retain(|d| d.name != "d-render-api");
        let v = check_architecture(&g);
        let r8: Vec<_> = v.iter().filter(|x| x.rule == "R-ARCH-8").collect();
        assert!(
            r8.len() >= 2,
            "expected cycle and multi-impl failures: {r8:?}"
        );
    }

    #[test]
    fn skill_integrity_checks_frontmatter_and_references() {
        let base = std::env::temp_dir()
            .join(format!("d-xtask-skill-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("skills/d/references")).unwrap();
        std::fs::create_dir_all(base.join("skills/d-dev")).unwrap();
        std::fs::write(
            base.join("skills/d/SKILL.md"),
            "---\nname: d\ndescription: demo\n---\nSee [ref](references/missing.md)\n",
        )
        .unwrap();
        std::fs::write(base.join("skills/d-dev/SKILL.md"), "no frontmatter")
            .unwrap();

        let mut v = Vec::new();
        check_skills_integrity(&base, &mut v);
        assert!(
            v.iter()
                .any(|x| x.message.contains("broken local skill reference"))
        );
        assert!(v.iter().any(|x| x.message.contains("YAML frontmatter")));

        std::fs::write(base.join("skills/d/references/missing.md"), "# ok\n")
            .unwrap();
        std::fs::write(
            base.join("skills/d-dev/SKILL.md"),
            "---\nname: d-dev\ndescription: dev\n---\n",
        )
        .unwrap();
        let mut clean = Vec::new();
        check_skills_integrity(&base, &mut clean);
        assert!(clean.is_empty(), "expected no skill violations: {clean:?}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
