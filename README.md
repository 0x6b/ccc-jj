# ccc-jj

A Jujutsu (jj) auto-committer CLI tool that uses Claude to generate commit messages from diffs.

## Description

`ccc-jj` is a standalone command-line tool that automatically generates commit messages for your Jujutsu workspace changes using Claude AI. It discovers your jj workspace, extracts diffs, generates appropriate commit messages via the Claude CLI, and creates commits.

## Features

- Automatic jj workspace discovery
- Diff extraction using `jj diff`
- Claude-powered commit message generation
- Conventional commits format
- Standalone tool (no external dependencies except `jj` and `claude` CLI)

## Prerequisites

- Rust toolchain (for building)
- [Jujutsu (jj)](https://github.com/martinvonz/jj) - Version control system
- [Claude CLI](https://github.com/anthropics/claude-cli) - For generating commit messages

## Installation

### From Source

```bash
cargo build --release
```

The binary will be available at `target/release/ccc-jj`.

### Installing Locally

```bash
cargo install --path .
```

## Usage

### Basic Usage

Run from within a jj workspace:

```bash
ccc-jj
```

### With Options

```bash
# Specify a custom workspace path
ccc-jj --path /path/to/workspace

# Use a different claude CLI binary
ccc-jj --claude-path /path/to/claude

# Combine options
ccc-jj --path /path/to/workspace --claude-path /usr/local/bin/claude
```

### Command-line Options

- `-p, --path <PATH>`: Path to the workspace (defaults to current directory)
- `-c, --claude-path <CLAUDE_PATH>`: Path to the claude CLI executable (defaults to "claude")
- `-h, --help`: Print help information
- `-V, --version`: Print version information

## How It Works

1. **Workspace Discovery**: Searches for a jj workspace starting from the specified directory
2. **Diff Extraction**: Runs `jj diff` to get the current changes
3. **Message Generation**: Calls Claude CLI with the diff to generate a conventional commit message
4. **Commit Creation**: Creates a new commit in jj with the generated message

## Example Workflow

```bash
# Make changes to files in your jj workspace
echo "new feature" >> src/main.rs

# Run ccc-jj to auto-commit
ccc-jj
```

Output:
```
Found workspace at: /Users/yourname/project
Getting diff...
Generating commit message using Claude...
Generated message: feat: add new feature to main module
Creating commit...
Committed change abc123def456 with message:
feat: add new feature to main module
```

## Configuration

### User Configuration

The tool automatically loads your existing jj configuration from standard locations:

- `~/.jjconfig.toml`
- `~/.config/jj/config.toml`

If you don't have jj configured yet, set it up with:

```bash
jj config set --user user.name "Your Name"
jj config set --user user.email "your.email@example.com"
```

The tool also automatically detects:
- `operation.hostname`: Your machine's hostname (via `whoami` crate)
- `operation.username`: Your username (via `whoami` crate or `$USER` environment variable)

### Claude CLI Configuration

The tool uses the Claude CLI's existing configuration. Ensure your Claude CLI is properly configured with API credentials before using this tool.

## Dependencies

This project depends on:

- `jj-lib`: Core Jujutsu library (from git repository)
- `tokio`: Async runtime
- `anyhow`: Error handling
- `clap`: Command-line argument parsing

## Development

### Building

```bash
cargo build
```

### Running Tests

```bash
cargo test
```

## License

See LICENSE file for details.

## Contributing

Contributions are welcome! Please open an issue or submit a pull request.

## Troubleshooting

### "Failed to load workspace"

Ensure you're running the command from within a jj workspace or specify a valid workspace path with `--path`.

### "jj diff failed"

Make sure the `jj` command is available in your PATH and the workspace is properly initialized.

### "claude CLI failed"

Verify that the Claude CLI is installed and properly configured with your API credentials. Test it by running `claude -p "test"` in your terminal.
