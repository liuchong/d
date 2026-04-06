#!/bin/bash
# Compare Rust and Zig project statistics

echo "=========================================="
echo "Project Comparison: Rust vs Zig"
echo "=========================================="
echo

cd "$(dirname "$0")"

echo "=== File Counts ==="
echo -n "Rust files (.rs): "
find d/crates -name "*.rs" -type f | wc -l

echo -n "Zig files (.zig): "
find chat.zig/src -name "*.zig" -type f 2>/dev/null | wc -l || echo "N/A"

echo
echo "=== Lines of Code ==="
echo "Rust (total):"
find d/crates -name "*.rs" -type f -exec wc -l {} + 2>/dev/null | tail -1

echo "Zig (total):"
find chat.zig/src -name "*.zig" -type f ! -path "*/test*" -exec wc -l {} + 2>/dev/null | tail -1 || echo "N/A"

echo
echo "=== Test Counts ==="
echo "Rust tests:"
cd d && cargo test --workspace 2>&1 | grep "test result:" | awk '{sum+=$3} END {print "  Total: " sum}'
cd ..

echo
echo "=== Module Comparison ==="
echo "Core modules in both projects:"
echo "  - agent/soul"
echo "  - tools"
echo "  - llm"
echo "  - session"
echo "  - context"
echo "  - security"
echo "  - workflow"
echo "  - mcp"
echo "  - rag"
echo "  - skills"
echo "  - personality"
echo "  - lsp"
echo "  - game"
echo
echo "Zig only:"
echo "  - plugin_api"
echo "  - benchmark"
echo "  - daemon (full)"
echo "  - cmdlet"
echo "  - worktree"
echo
echo "Rust only:"
echo "  - http (server)"
echo "  - memory (embeddings)"

echo
echo "=========================================="
echo "Run cross-project tests:"
echo "  cd d && cargo test --workspace"
echo "  cd chat.zig && zig build test"
echo "=========================================="
