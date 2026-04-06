# Features

## Core Tools (11)

| Tool | Description |
|------|-------------|
| read_file | Read file contents |
| write_file | Write content to file |
| str_replace | Replace text in file |
| list_directory | List directory contents |
| glob | Find files by pattern |
| grep | Search file contents |
| shell | Execute shell commands |
| git | Execute git commands |
| ask_user | Interactive user prompts |
| web_search | DuckDuckGo search |
| fetch_url | Fetch URL content |

## Agent System

- **ReAct Loop**: Reasoning and acting cycle
- **Planner**: Task planning and decomposition
- **Tool Registry**: Dynamic tool management
- **Cost Tracking**: Token and cost monitoring

## Advanced Features

### Security
- 18 security rules for dangerous patterns
- Approval system for sensitive operations
- Audit logging

### Session Management
- Persistent sessions
- Export/Import (JSON, Markdown, HTML, Text)
- Multi-session support

### Context Management
- Token estimation
- Message compaction
- Context window management

### RAG (Retrieval-Augmented Generation)
- Text chunking (sentence, paragraph, code)
- Vector indexing
- Semantic search

### MCP (Model Context Protocol)
- MCP client implementation
- Tool discovery
- Resource management

### Workflow Engine
- Step-based workflows
- Condition evaluation
- Error handling
- Loop support

### Skills System
- Skill tree organization
- Proficiency levels
- Usage tracking

### LSP Support
- Language Server Protocol client
- Completion, hover, definition
- Multi-language support

### Personality Engine
- User preference learning
- Communication style adaptation
- Feedback tracking

### Self-Correction
- Error pattern recognition
- Retry strategies
- Error memory

### Thinking Mode
- 6 levels of reasoning depth
- Token budget management
- Auto-enable for complex queries

### Background Tasks
- Shell commands
- File watching
- Agent tasks
- HTTP requests

### Pattern Recognizer
- Command sequence detection
- Time-based patterns
- Tool usage patterns
- Smart suggestions

### Text Adventure Game
- 5 rooms with navigation
- Item system
- Win conditions

### Smart Completion
- Slash command completion
- Tool name completion
- File path completion
- History-based suggestions

### Benchmark Framework
- Performance measurement
- Regression detection
- Benchmark suites

### Daemon Mode
- Background operation
- PID file management
- Signal handling
- Client-server IPC

### Plugin System
- Plugin loading/unloading
- Hook system
- Capability-based permissions
- Isolated contexts

## CLI Features

- Interactive REPL
- Color support
- History navigation
- Configuration management
- HTTP server mode

## Development

- **Tests**: 184+ tests
- **Coverage**: Core functionality covered
- **Documentation**: Inline docs for public APIs
