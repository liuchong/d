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
├── src/           # Source code
├── tests/         # Test files
├── docs/          # Documentation
├── examples/      # Example code
└── AGENTS.md      # This file
```

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
