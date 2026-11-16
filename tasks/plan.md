# Prototype implementation plan

1. Create a private tg crate and copy the architecture into repository documentation.
2. Specify JSON send_message, validation and ambiguous timeout as failing localhost mock tests; implement the shared request path and typed errors.
3. Add callback buttons, callback Update decoding and answer_callback_query with mock tests.
4. Add streaming send_document from paths, bytes and file IDs; test multipart bytes and local size/I/O failures.
5. Review the security boundary, add failure-path regressions, runnable examples and documentation; run formatting, Clippy, tests, rustdoc and dependency audit.

The shared request path is the main risk: no transport or application retry, no raw error leakage, deadline includes response consumption, and body allocation is bounded. Each feature builds on this path. Keep only the main library package until another executable is implemented.
