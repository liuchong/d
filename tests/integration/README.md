# Integration Tests - Cross-Project Validation

These tests validate real-world use cases that should work identically in both Rust and Zig versions.

## Test Categories

1. **Tool Execution Tests** - Verify tools work correctly
2. **Session Management Tests** - Persistence, export/import
3. **Security Tests** - Verify security rules catch dangerous patterns
4. **Workflow Tests** - Complex multi-step operations
5. **Performance Tests** - Benchmark comparisons

## Usage

```bash
# Run all integration tests
cargo test --test integration

# Run specific test category
cargo test --test integration tool_tests
```

## Test Alignment with Zig

Each test has a corresponding test in the Zig project:
- Rust: `tests/integration/tool_tests.rs`
- Zig: `tests/integration/tool_integration.zig`

Expected results should match between implementations.
