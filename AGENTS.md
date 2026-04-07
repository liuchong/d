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

Maintain appropriate tests for your changes following the test hierarchy:

```
tests/
├── unit/            # Unit tests: test business functions, methods, classes
│                   # No external dependencies, run independently
├── integration/     # Integration tests: verify module/service/component collaboration
│                   # Involves databases, caches, models, internal APIs
├── e2e/             # End-to-end tests: simulate real user complete workflows
│                   # Full system black-box testing, focus on usability
└── spike/           # Spike tests: verify external services, third-party APIs
                    # Infrastructure, language features, technical feasibility
                    # No business logic
```

**Test Categories:**

- **Unit Tests**: Test individual functions/modules in isolation, no external dependencies
- **Integration Tests**: Verify components work together, may use real dependencies
- **End-to-End Tests**: Test complete user workflows, full system testing
- **Spike Tests**: Verify external services, APIs, infrastructure feasibility

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
├── tests/         # Test files (see Testing section for hierarchy)
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
