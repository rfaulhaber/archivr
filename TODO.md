# archivr v0.1.0 Pre-Release TODO

## Critical — Fix Before Release

- [x] **Template parse failure silently swallowed** — Fixed: `with_template` now returns `Result`, propagating parse errors.

- [x] **Invalid `--before`/`--after` dates silently ignored** — Fixed: `get_timestamp` now returns `Result` with a clear error message.

- [x] **Rate limit aborts with no resume hint** — Fixed: error message now mentions `--resume`.

- [x] **CLAUDE.md describes non-existent features** — Fixed: replaced `LastRun`/`--incremental` docs with `--resume` documentation.

- [x] **No LICENSE file** — Fixed: added GPL-3.0 LICENSE file.

## Important — Should Fix Soon

- [x] **`OAuthScope::Write` requested unnecessarily** — Fixed: removed `OAuthScope::Write`, now only requests `Basic` and `Offline`.

- [x] **`--save-images` skips images without `has_original_dimensions`** — Fixed: falls back to first available media URL.

- [x] **`newest_timestamp` return value discarded** — Fixed: removed dead code; `run_backup` now returns `()`.

- [x] **README missing most CLI flags** — Fixed: added CLI flags table, documented `newer_href` template variable.

- [x] **No validation that `blog_name` is non-empty** — Fixed: added `value_parser` validation and help text in `cmd.rs`.

- [x] **README structure** — Fixed: moved "Planned features" to end of file, after template example.

- [ ] **No CI** — No GitHub Actions for clippy/test/build. Strict lint rules are only enforced manually.

- [ ] **Very few tests** — Only 3 template tests. No coverage for config resolution, timestamp parsing, job serialization, image URL collection, or OAuth parsing.

## Low Priority / Nice to Have

- [ ] **`clippy.toml` has Dioxus-specific rules** — Irrelevant `generational_box`/`dioxus_signals` entries should be removed.

- [ ] **Token refresh error discarded** (`auth.rs` ~line 241) — `_e` should be included in the log message.

- [ ] **`NoConsumerKeyAndSecret` error imprecise** — Same message for missing key vs. missing secret.

- [ ] **`blog_name` positional arg has no help text** (`cmd.rs`)

- [ ] **`tokio` features = `["full"]`** — More than needed for a CLI tool; could slim down to specific features.

- [ ] **Missing `Cargo.toml` metadata** — `description`, `license`, `repository`, etc. needed if publishing to crates.io.
