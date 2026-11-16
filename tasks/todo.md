# Prototype tasks

- [x] Bootstrap crate and local architecture. Verify: Cargo manifest and source document.
- [x] Message and failure semantics. Verified: exact JSON and typed response; one request on timeout; local validation sends nothing. cargo test --test messages: 3 passed.
- [x] Callback slice. Verified: inline keyboard JSON, Update decoding and answer callback. cargo test --test callbacks: 2 passed; message regressions passed; Clippy clean.
- [ ] File slice. Accept: bytes/path streaming and file ID; upload bounds before dispatch. Depends on request path. Verify: cargo test --test documents.
- [ ] Review and handoff. Accept: security regressions, examples and honest scope documentation. Verify: format, Clippy, all tests, docs and dependency audit.
