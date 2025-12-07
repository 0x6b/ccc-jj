# jc

A Jujutsu (jj) auto-committer CLI tool that uses Claude to generate commit messages from diffs.

## Description

`jc` is a standalone command-line tool that automatically generates commit messages for your Jujutsu workspace changes using Claude AI. It discovers your jj workspace, extracts diffs, generates appropriate commit messages via the Claude CLI, and creates commits.

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

### Installing Locally

```console
$ cargo install --git https://github.com/0x6b/ccc-jj
```

## Usage

### Basic Usage

Run from within a jj workspace:

```bash
$ jc --help
Auto-commit changes in a jj workspace using Claude for commit messages

Usage: jc [OPTIONS]

Options:
  -p, --path <PATH>          Path to the workspace (defaults to current directory)
  -l, --language <LANGUAGE>  Language to use for commit messages [env: CCC_JJ_LANGUAGE=] [default: English]
  -m, --model <MODEL>        Model to use for generating a commit message [env: CCC_JJ_MODEL=] [default: haiku]
  -h, --help                 Print help
  -V, --version              Print version
```

## How It Works

1. Workspace Discovery: Searches for a jj workspace starting from the specified directory
2. Change Detection: Snapshots the working copy and compares its tree with the parent commit's tree to detect actual changes
3. Diff Extraction: Runs `jj diff` to get the current changes for message generation
4. Message Generation: Calls Claude CLI with the diff to generate a conventional commit message
5. Commit Creation: Creates a new commit in jj with the generated message

The tool intelligently prevents duplicate commits by comparing tree IDs - if the working copy tree matches the parent commit's tree, no commit is created. This handles jj's behavior of automatically creating new working-copy commits after each commit.

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

## License

MIT. See [LICENSE](./LICENSE) for details.
