//! Workspace metadata extraction from `cargo metadata --format-version 1 --no-deps`.

use crate::json::{self, JsonValue};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Kernel,
    Contracts,
    Components,
    Entries,
    Assemblies,
    Tools,
}

impl Layer {
    pub const ALL: [Layer; 6] = [
        Layer::Kernel,
        Layer::Contracts,
        Layer::Components,
        Layer::Entries,
        Layer::Assemblies,
        Layer::Tools,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::Contracts => "contracts",
            Self::Components => "components",
            Self::Entries => "entries",
            Self::Assemblies => "assemblies",
            Self::Tools => "tools",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Build,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepEdge {
    pub name: String,
    pub kind: DepKind,
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    pub manifest_dir: String,
    pub deps: Vec<DepEdge>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchConfig {
    pub kernel: Vec<String>,
    pub contracts: Vec<String>,
    pub components: Vec<String>,
    pub entries: Vec<String>,
    pub assemblies: Vec<String>,
    pub tools: Vec<String>,
    pub zero_external_deps: Vec<String>,
    pub minimal_assembly: Option<String>,
    pub minimal_forbidden: Vec<String>,
    pub multi_impl_contracts: Vec<String>,
}

impl ArchConfig {
    pub fn members_for(&self, layer: Layer) -> &[String] {
        match layer {
            Layer::Kernel => &self.kernel,
            Layer::Contracts => &self.contracts,
            Layer::Components => &self.components,
            Layer::Entries => &self.entries,
            Layer::Assemblies => &self.assemblies,
            Layer::Tools => &self.tools,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceGraph {
    pub root: PathBuf,
    pub arch: ArchConfig,
    pub packages: BTreeMap<String, PackageInfo>,
}

impl WorkspaceGraph {
    pub fn load_live() -> Result<Self, String> {
        let mut cmd = Command::new("cargo");
        cmd.args(["metadata", "--format-version", "1", "--no-deps"]);
        let out = cmd
            .output()
            .map_err(|e| format!("failed to run `cargo metadata`: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!("`cargo metadata` failed: {stderr}"));
        }
        let stdout = String::from_utf8(out.stdout)
            .map_err(|e| format!("cargo metadata stdout is not UTF-8: {e}"))?;
        Self::from_metadata_json(&stdout)
    }

    pub fn from_metadata_json(raw: &str) -> Result<Self, String> {
        let val = json::parse(raw)?;
        let root_str = val
            .get("workspace_root")
            .and_then(JsonValue::as_str)
            .ok_or("missing `workspace_root` in cargo metadata")?;
        let root = PathBuf::from(root_str);

        let arch_val = val
            .get("metadata")
            .and_then(|m| m.get("architecture"))
            .ok_or("missing `[workspace.metadata.architecture]` in root Cargo.toml")?;
        let arch = parse_arch_config(arch_val)?;

        let member_ids: BTreeSet<&str> = val
            .get("workspace_members")
            .and_then(JsonValue::as_array)
            .ok_or("missing `workspace_members`")?
            .iter()
            .filter_map(JsonValue::as_str)
            .collect();

        let raw_pkgs = val
            .get("packages")
            .and_then(JsonValue::as_array)
            .ok_or("missing `packages`")?;

        let mut workspace_names = BTreeSet::new();
        for pkg in raw_pkgs {
            let Some(id) = pkg.get("id").and_then(JsonValue::as_str) else {
                continue;
            };
            if member_ids.contains(id)
                && let Some(name) = pkg.get("name").and_then(JsonValue::as_str)
            {
                workspace_names.insert(name.to_string());
            }
        }

        let mut packages = BTreeMap::new();
        for pkg in raw_pkgs {
            let Some(id) = pkg.get("id").and_then(JsonValue::as_str) else {
                continue;
            };
            if !member_ids.contains(id) {
                continue;
            }
            let name = pkg
                .get("name")
                .and_then(JsonValue::as_str)
                .ok_or("package missing `name`")?
                .to_string();
            let manifest_path = pkg
                .get("manifest_path")
                .and_then(JsonValue::as_str)
                .ok_or("package missing `manifest_path`")?;
            let manifest_dir = relative_dir(&root, Path::new(manifest_path));

            let deps_arr = pkg
                .get("dependencies")
                .and_then(JsonValue::as_array)
                .ok_or("package missing `dependencies`")?;
            let mut deps = Vec::with_capacity(deps_arr.len());
            for d in deps_arr {
                let dep_name = d
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .ok_or("dependency missing `name`")?
                    .to_string();
                let kind = match d.get("kind") {
                    None | Some(JsonValue::Null) => DepKind::Normal,
                    Some(JsonValue::Str(s)) if s == "normal" => DepKind::Normal,
                    Some(JsonValue::Str(s)) if s == "build" => DepKind::Build,
                    Some(JsonValue::Str(s)) if s == "dev" => DepKind::Dev,
                    Some(other) => {
                        return Err(format!(
                            "unexpected dependency kind: {other:?}"
                        ));
                    }
                };
                let has_source =
                    matches!(d.get("source"), Some(JsonValue::Str(_)));
                let in_workspace = workspace_names.contains(&dep_name);
                let external = has_source || !in_workspace;
                deps.push(DepEdge {
                    name: dep_name,
                    kind,
                    external,
                });
            }

            packages.insert(
                name.clone(),
                PackageInfo {
                    name,
                    manifest_dir,
                    deps,
                },
            );
        }

        Ok(Self {
            root,
            arch,
            packages,
        })
    }
}

fn parse_string_list(
    obj: &JsonValue,
    key: &str,
    required: bool,
) -> Result<Vec<String>, String> {
    let Some(val) = obj.get(key) else {
        if required {
            return Err(format!("missing `architecture.{key}`"));
        }
        return Ok(Vec::new());
    };
    let arr = val
        .as_array()
        .ok_or_else(|| format!("`architecture.{key}` must be an array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str().ok_or_else(|| {
            format!("`architecture.{key}` items must be strings")
        })?;
        out.push(s.to_string());
    }
    Ok(out)
}

fn parse_arch_config(val: &JsonValue) -> Result<ArchConfig, String> {
    let minimal_assembly = match val.get("minimal_assembly") {
        None | Some(JsonValue::Null) => None,
        Some(JsonValue::Str(s)) => Some(s.clone()),
        _ => {
            return Err(
                "`architecture.minimal_assembly` must be a string".into()
            );
        }
    };
    Ok(ArchConfig {
        kernel: parse_string_list(val, "kernel", true)?,
        contracts: parse_string_list(val, "contracts", true)?,
        components: parse_string_list(val, "components", true)?,
        entries: parse_string_list(val, "entries", true)?,
        assemblies: parse_string_list(val, "assemblies", true)?,
        tools: parse_string_list(val, "tools", true)?,
        zero_external_deps: parse_string_list(val, "zero_external_deps", true)?,
        minimal_assembly,
        minimal_forbidden: parse_string_list(val, "minimal_forbidden", false)?,
        multi_impl_contracts: parse_string_list(
            val,
            "multi_impl_contracts",
            false,
        )?,
    })
}

fn relative_dir(root: &Path, manifest: &Path) -> String {
    let dir = manifest.parent().unwrap_or(manifest);
    let rel = dir.strip_prefix(root).unwrap_or(dir);
    let s = rel.to_string_lossy().replace('\\', "/");
    if s == "." { String::new() } else { s }
}
