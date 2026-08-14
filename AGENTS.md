# Agent Guidelines

This document provides guidelines for AI agents working on this repository.

## Code Quality Requirements

### 1. Code Formatting

**All code must be properly formatted before committing.**

- Run the appropriate formatter for your language:
  - **Rust**: `cargo fmt`
  - **Go**: `gofmt -w .`
  - **Python**: `black .` or `ruff format .`
  - **JavaScript/TypeScript**: `prettier --write .`
  - **Zig**: `zig fmt`
  - **C/C++**: `clang-format -i`
  - **Emacs Lisp**: `emacs --batch --eval '(indent-region (point-min) (point-max) nil)' -f save-buffer` (or use `elisp-format` if available)
- Ensure consistent indentation (spaces preferred, matching project style)
- Remove trailing whitespace from all lines
- Ensure files end with exactly one newline

### 2. Testing

Maintain appropriate tests for your changes:

- **Unit Tests**: Test individual functions/modules in isolation
- **Integration Tests**: Verify components work together
- **End-to-End Tests**: Test complete workflows (if applicable)

Run the test suite before committing:
```bash
# Example commands - adjust for your project
make test
# or
cargo test
# or
pytest
```

### 3. Code Review Checklist

Before submitting changes:

- [ ] Code is formatted correctly
- [ ] No trailing whitespace or inconsistent indentation
- [ ] Tests pass locally
- [ ] No compiler warnings (or all warnings are addressed)
- [ ] Documentation updated if needed
- [ ] Commit messages are clear and descriptive

## Project Structure

Respect the existing project structure:

```
.
├── src/           # Source code (httpd/ 为核心服务器模块目录)
├── tests/         # Integration tests（cargo test，oneshot 方式）
├── examples/      # 使用示例集（见 examples/README.md）
├── scripts/       # acceptance.sh 等工程脚本
├── .github/       # GitHub Actions CI
└── AGENTS.md      # This file
```

## Acceptance

除 `cargo test` 外，发布前需跑端到端验收脚本：

```bash
scripts/acceptance.sh   # 构建 + 启动真实服务 + 全套 HTTP 行为检查
```

新功能开发时，需同步在 `scripts/acceptance.sh` 中追加对应验收项，
并在 `examples/` 中补充使用示例。

## Language-Specific Notes

### General Principles

- Prefer explicit error handling
- Document complex logic with comments
- Use meaningful variable and function names
- Keep functions focused and concise
- Avoid unnecessary dependencies

### Commit Style

- Use present tense ("Add feature" not "Added feature")
- Use imperative mood ("Move cursor to..." not "Moves cursor to...")
- Keep the first line under 50 characters
- Reference issues when applicable

## Communication

- Ask questions if requirements are unclear
- Explain the reasoning behind significant changes
- Keep the user informed of progress on complex tasks
