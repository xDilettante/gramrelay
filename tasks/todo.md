# Prototype tasks

- [x] Bootstrap crate and local architecture. Verify: Cargo manifest and source document.
- [x] Message and failure semantics. Verified: exact JSON and typed response; one request on timeout; local validation sends nothing. cargo test --test messages: 3 passed.
- [ ] Callback slice. Accept: inline keyboard JSON, Update decoding and answer callback. Depends on message path. Verify: cargo test --test callbacks.
- [ ] File slice. Accept: bytes/path streaming and file ID; upload bounds before dispatch. Depends on request path. Verify: cargo test --test documents.
- [ ] Review and handoff. Accept: security regressions, examples and honest scope documentation. Verify: format, Clippy, all tests, docs and dependency audit.
