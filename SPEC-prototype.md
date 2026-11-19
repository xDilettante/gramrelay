# Bot API prototype

Implements the first vertical slice of [the architecture](docs/ARCHITECTURE.md).

## Contract

- Lazy awaitable builders: send_message, answer_callback_query, send_document.
- Typed IDs; plain text by default; inline callback buttons and callback Update decoding.
- Pooled reqwest client with rustls. No automatic retries, redirects or ambient proxies.
- A configurable deadline covers preparation, upload and response reading. After dispatch, timeout, transport failure, malformed response and 5xx mean Unknown; local failures mean NotSent. A valid explicit Bot API refusal means Rejected.
- Stream files from disk; support byte buffers and existing file IDs. Bound uploads, response bytes and concurrent requests. Fail fast on overload.
- Do not expose tokens, request URLs, message text or remote descriptions through Error Display, Debug or source chains.

## Verification

Local HTTP mocks exercise actual JSON/multipart encoding and response decoding. Verify exactly one received request after an ambiguous timeout, callback handling, file contents, bounded responses, local validation, refusal parameters, redirects and redaction. No Telegram token or external chat is needed.

Commands: cargo fmt --all -- --check; cargo clippy --all-targets -- -D warnings; cargo test --all-targets; cargo test --doc; cargo doc --no-deps.

## Boundaries

Always keep changes scoped, document actual guarantees and test failure paths. Never read real bot credentials for tests, send live test messages, publish a crate or push to a remote as part of this prototype. Polling, webhooks, routing, persistent journals, CLI/MCP, codegen and MTProto follow in later slices. Callback input is decoded from an already received update; no receiver is claimed yet.
