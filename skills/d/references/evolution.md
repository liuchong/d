# Self-Evolving Usage Protocol

`d` is built as a pure-Rust layered workspace where new contracts, component implementations, config fields, and assemblies can be added over time without breaking existing MCP workflows.

## How This Skill Stays Current Across Releases

1. **Runtime Discovery Overrides Static Tables**:
   The tables in this skill reflect baseline components (`l4-transport`, `proto-http`, `ext-compression`, `ext-cors`, `app-files`, `doc-render`, `doc-render-plain`, `viewer-markdown`, `viewer-code`, `viewer-media`). Whenever connected to a running `d` instance, treat **`d_catalog`** as the authoritative source for:
   - Which plugins exist in the current binary (`d`, `d-minimal`, or custom assemblies)
   - Required vs optional service dependencies (`required_deps`, `optional_deps`)
   - Validated configuration fields (`fields`), types, nullability, sensitivity flags, and defaults
2. **Swap Equivalent Implementations Without Changing Callers**:
   When two plugins implement the same contract (e.g., `doc-render` vs `doc-render-plain` for `render-api`), inspect `d_catalog` and swap via `d_apply_patches` (disable/remove one, insert the other) while keeping upstream viewers or `app-files` intact.
3. **Graceful Degradation Under Policy Constraints**:
   - Check `d_server_info` first: if `mcp.read_only == true`, only invoke read/probe tools and guide the user cleanly if mutation is needed.
   - If `mcp.allow_shutdown == false`, avoid calling `d_shutdown` and advise restarting or using `--mcp-allow-shutdown`.
   - If `--no-http` was passed (`local_addr == null`), check `d_http_probe` for route verification and offer binding `proto-http.bind_addr` when TCP serving is desired.
4. **Update Discipline for Contributors**:
   Whenever adding a new plugin or config option in the repository, follow `skills/d-dev/SKILL.md` so `Plugin::schema()` + `validate_config()` automatically publish the new capability to `d_catalog` and MCP clients.
