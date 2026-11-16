# Prototype tasks

- [x] Bootstrap crate and local architecture. Verify: Cargo manifest and source document.
- [x] Message and failure semantics. Verified: exact JSON and typed response; one request on timeout; local validation sends nothing. cargo test --test messages: 3 passed.
- [x] Callback slice. Verified: inline keyboard JSON, Update decoding and answer callback. cargo test --test callbacks: 2 passed; message regressions passed; Clippy clean.
- [x] File slice. Verified: bytes/path streaming and file ID; upload bounds before dispatch. cargo test --test documents: 4 passed; all 9 integration tests and Clippy passed.
- [x] Review and handoff. Verified on Windows / Rust 1.98.0: 23 unit/integration tests, 2 doctests, all 4 examples compile, offline mock example runs; Clippy and rustdoc pass; cargo audit finds 0 vulnerabilities and no warnings. README records limits and deferred features.
