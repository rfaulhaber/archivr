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

- `main.rs` - CLI entry point, argument parsing, authentication flow orchestration
- `lib.rs` - Core library: error types, OAuth callback server, re-exports
- `cmd.rs` - CLI argument definitions using clap derive
- `config.rs` - Configuration file deserialization

The OAuth flow uses a local TCP server on port 6263 to capture the callback.

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
