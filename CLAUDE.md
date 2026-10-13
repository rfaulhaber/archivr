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

All Rust sources live under `src/`:

- `src/main.rs` - CLI entry point: exit-code mapping, Ctrl-C handling, per-blog orchestration (`backup_blog`), the cursor-pagination loop (`run_backup`), single-post mode, special collections (drafts/queue/submissions/likes), avatar backup
- `src/lib.rs` - Core library: error types, OAuth callback server, re-exports
- `src/cmd.rs` - CLI argument definitions using clap derive (manual `Debug` impl redacts OAuth credentials)
- `src/config.rs` - Configuration file deserialization and CLI args resolution into one `ResolvedConfig` per requested blog
- `src/auth.rs` - Authentication: token storage, expiry checking, CSRF verification, interactive OAuth flow, and `Session` (the shared, renewable credential/client handle every API call goes through; tokens are refreshed mid-run, since a rate-limit sleep can outlive one)
- `src/api.rs` - Shared request policy: per-request timeout, transient-error retries, rate-limit sleeps, token renewal on expiry or 401 (`send_with_retries` over a `ClientSource`), error classification for exit codes, stdout/stderr writers (`say`/`say_error` — the sanctioned alternative to the denied `println!`/`eprintln!`)
- `src/job.rs` - `JobState` (in-progress backup pagination cursor and persisted job parameters for `--resume`)
- `src/state.rs` - `BackupState` (`.archivr-state.json`: incremental high-water mark plus the filter descriptor it was recorded under)
- `src/media.rs` - Media collection/download/URL-rewriting into the shared `media/{images,videos,audio}/` stores, Wayback Machine fallback, the yt-dlp bridge for external embeds, `media-list.jsonl` writer
- `src/template.rs` - minijinja rendering: default template, NPF block rendering, HTML escaping/URL sanitizing, `--no-embeds` stripping
- `src/index.rs` - `ArchiveIndex` (`.archivr-index.json` metadata and `index.html` generation)
- `src/notes.rs` - Notes fetching with `_links` cursor pagination
- `src/fsutil.rs` - Atomic writes (temp + rename), mtime stamping, the output-directory lock

The OAuth flow uses a local TCP server on port 6263 to capture the callback. Authentication tokens are persisted to `{data_local_dir}/archivr/auth.json` using the `directories` crate.

The `crabrave` library handles Tumblr API interactions.

Exit codes are part of the CLI contract (documented in the README): 0 archived something, 1 completed but archived nothing, 2 usage error, 3 interrupted, 4 API/network error, 5 other runtime error.

### Resumable backups

`--resume` mode uses a `.archivr-job.json` file in the output directory to persist pagination state and the original job parameters (filters, output format, etc.). On resume, these saved parameters are restored so the backup continues with the same query regardless of the current CLI flags.

### Incremental backups

`--incremental` mode uses a `.archivr-state.json` file in the output directory to persist the timestamp of the newest post archived in the last successful run. On the next incremental run, this timestamp is passed to the Tumblr API as the `after` filter so only newer posts are fetched. The state file outlives runs (unlike `.archivr-job.json`, which is deleted on success). First-time use with no prior state falls back to a full backup to establish the baseline. The flag conflicts with `--resume` and `--after`.

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
