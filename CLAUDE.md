# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

archivr is a Tumblr backup tool written in Rust. It uses OAuth2 to authenticate with Tumblr's API and backs up blog content. The project uses Rust 2024 edition.

## Build Commands

```bash
cargo build              # Build the project
cargo run -- <args>      # Run with arguments
cargo test               # Run tests
cargo nextest run        # Run tests with nextest (preferred, available in devshell)
cargo clippy             # Run linter (strict rules enforced)
```

## Development Environment

This project uses Nix flakes for development. Enter the devshell with:
```bash
nix develop
# or with direnv: direnv allow
```

The devshell provides: rust toolchain, clippy, rust-analyzer, cargo-nextest.

## Code Architecture

There is no `src/` directory in this project. All the Rust source files start at the top level.

- `main.rs` - CLI entry point, authentication flow, backup orchestration (full and incremental)
- `lib.rs` - Core library: error types, OAuth callback server, re-exports
- `cmd.rs` - CLI argument definitions using clap derive
- `config.rs` - Configuration file deserialization and CLI args resolution into `ResolvedConfig`
- `auth.rs` - Authentication: token storage, expiry checking, refresh-on-expiry, CSRF verification, interactive OAuth flow
- `job.rs` - `JobState` (in-progress backup pagination) and `LastRun` (incremental backup marker)

The OAuth flow uses a local TCP server on port 6263 to capture the callback. Authentication tokens are persisted to `{data_local_dir}/archivr/auth.json` using the `directories` crate.

The `crabrave` library handles Tumblr API interactions.

### Incremental backups

`--incremental` mode uses a `.archivr-last-run.json` marker file in the output directory to track the newest post timestamp from the last successful run. On incremental runs, the backup loop stops as soon as it encounters a post at or before that timestamp. The marker is saved after every successful backup (full or incremental) so switching between modes works seamlessly.

## Strict Linting Rules

This project has aggressive clippy rules (`Cargo.toml` + `clippy.toml`):

**Forbidden patterns (will fail CI):**
- `unwrap()`, `expect()`, `panic!()`, `todo!()`, `unimplemented!()`
- `dbg!()`, `print!()`, `println!()`, `eprint!()`, `eprintln!()`
- `std::thread::sleep` (use `tokio::time::sleep`)
- async locks (`tokio::sync::Mutex/RwLock`) - use channels instead

**Required alternatives:**
- `std::path::Path/PathBuf` → `camino::Utf8Path/Utf8PathBuf`
- `std::fs::*` → `fs_err::*` (for better error messages)
- `std::sync::Mutex/RwLock` → `parking_lot::*`
- `std::net::TcpStream` → `tokio::net::TcpStream`

**Unused variables:** Prefix with underscore (e.g., `_e`) to silence warnings.
