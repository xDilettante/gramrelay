# Telegram Rust

This repository is a private Bot API prototype; crate name tg is provisional. Project name for reports: Telegram Rust.

- Read SPEC-prototype.md before changing behavior; docs/ARCHITECTURE.md describes the longer-term design.
- Keep lazy request builders and one shared request/response path. Do not add automatic retries for ambiguous writes.
- Preserve NotSent, Rejected and Unknown semantics. Cancellation after polling a future can leave an unknown remote effect.
- Never retain raw reqwest errors, credential-bearing URLs or remote descriptions in public diagnostic chains.
- Use localhost mocks and synthetic credentials for tests. Run cargo test --locked --all-targets, cargo test --locked --doc, cargo fmt --all -- --check and cargo clippy --locked --all-targets -- -D warnings.
- Keep Cargo.lock checked in. Before delivery, run cargo audit.
- Implement only the requested slice; no live Telegram calls, publishing or remote pushes unless explicitly requested.
