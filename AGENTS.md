# GramRelay

This repository develops the Telegram SDK described in docs/ARCHITECTURE.md. The Bot API crate is named tg. Project name for reports: GramRelay.

- Read the active module spec and tasks/todo.md before changing behavior; SPEC-prototype.md records the completed initial slice.
- Keep lazy request builders and one shared request/response path. Do not add automatic retries for ambiguous writes.
- Preserve NotSent, Rejected and Unknown semantics. Cancellation after polling a future can leave an unknown remote effect.
- Never retain raw reqwest errors, credential-bearing URLs or remote descriptions in public diagnostic chains.
- Use localhost mocks and synthetic credentials for tests. Run cargo test --locked --all-targets, cargo test --locked --doc, cargo fmt --all -- --check and cargo clippy --locked --all-targets -- -D warnings.
- Keep Cargo.lock checked in. Before delivery, run cargo audit.
- Schema changes: read SPEC-protocol.md, run python -m unittest discover -s tools/tests -v and python tools/schema.py check. Never hand-edit generated Rust; generation is developer-only and offline.
- Implement only the requested slice; no live Telegram calls, publishing or remote pushes unless explicitly requested.
- Check free disk space and generated target size before each full build matrix and after each major verification stage. Keep target within 20 GiB; clean completed standalone SDK/experiment build caches regularly with cargo clean, retaining only current binaries and small verification logs. Do not start another large build with less than 20 GiB free. Never clean a target while its build/test process is live. Repository Cargo config disables incremental snapshots and keeps only line-table debug information; do not override it with full debug info without a concrete need and cleanup plan.
