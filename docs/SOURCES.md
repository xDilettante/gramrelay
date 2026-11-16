# Implementation sources

The implementation uses reqwest 0.13.5, Tokio 1.54.1, tokio-util 0.7.20 and wiremock 0.6.5 as pinned by Cargo.lock. Telegram contracts were checked against the Bot API documentation on 16 November 2026. The structural schema pins all 193 methods and 425 types in Bot API 10.4. Callable coverage is tracked separately in tasks/todo.md.

- Telegram sendMessage and plain-text parameters: https://core.telegram.org/bots/api#sendmessage
- Callback answer: https://core.telegram.org/bots/api#answercallbackquery
- Callback button byte limit: https://core.telegram.org/bots/api#inlinekeyboardbutton
- Document upload: https://core.telegram.org/bots/api#senddocument
- Error parameters: https://core.telegram.org/bots/api#responseparameters
- Polling parameters and acknowledgement: https://core.telegram.org/bots/api#getupdates
- Bot identity: https://core.telegram.org/bots/api#getme
- Webhook status: https://core.telegram.org/bots/api#getwebhookinfo
- Explicitly disable transport retries: https://docs.rs/reqwest/0.13.5/reqwest/retry/fn.never.html
- Multipart streaming with known length: https://docs.rs/reqwest/0.13.5/reqwest/multipart/struct.Part.html#method.stream_with_length
- Pool reuse: https://docs.rs/reqwest/0.13.5/reqwest/#making-a-get-request
- Bounded file reading: https://docs.rs/tokio/latest/tokio/io/trait.AsyncReadExt.html#method.take
- AsyncRead to Stream: https://docs.rs/tokio-util/latest/tokio_util/io/struct.ReaderStream.html
- Local HTTP mocks: https://docs.rs/wiremock/0.6.5/wiremock/
- Cancellation tokens: https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html
- Process signal handling: https://docs.rs/tokio/latest/tokio/signal/fn.ctrl_c.html
- Serde enum representations and ordered matching of untagged variants: https://serde.rs/enum-representations.html
- Serde preservation of additional fields: https://serde.rs/attr-flatten.html
- Entity offsets and formatting rules: https://core.telegram.org/bots/api#messageentity and https://core.telegram.org/bots/api#formatting-options
- Quote/code nesting exception and entity categories: https://github.com/tdlib/td/blob/master/td/telegram/MessageEntity.cpp
- Code-point message/caption limits for bots: https://github.com/tdlib/td/blob/master/td/telegram/MessageContent.cpp (utf8_length checks, inspected 16 November 2026).
- Ephemeral receiver scope, callback replies and short-lived reply IDs: https://core.telegram.org/bots/api#ephemeralmessageparameters and https://core.telegram.org/bots/api#replyparameters
- Guest response operation: https://core.telegram.org/bots/api#answerguestquery
- File endpoint and cloud download limit: https://core.telegram.org/bots/api#getfile
- URL segment encoding: https://docs.rs/url/2.5.8/url/struct.PathSegmentsMut.html#method.extend
- Caller-owned async writers, including Vec: https://docs.rs/tokio/1.54.1/tokio/io/trait.AsyncWrite.html
- Non-cryptographic retry jitter: fastrand 2.5.0, pinned in Cargo.lock; f64's 0..1 contract verified in the installed registry source (src/lib.rs). https://docs.rs/fastrand/2.5.0/fastrand/

- Optional tracing spans and async context: https://docs.rs/tracing/0.1.44/tracing/instrument/trait.Instrument.html (no enter guard is held across await).
- Optional metrics facade, counters/histograms and application-owned recorder: https://docs.rs/metrics/0.24.6/metrics/
- Test-kit response conversion shares reqwest decoding: https://docs.rs/reqwest/0.13.5/reqwest/struct.Response.html#impl-From%3CResponse%3CT%3E%3E-for-Response
- Test-kit request frame consumption: https://docs.rs/http-body-util/0.1.3/http_body_util/trait.BodyExt.html#method.frame
- Cancellable synthetic chunk streams: https://docs.rs/futures-util/0.3.34/futures_util/stream/fn.unfold.html
- SQLite transactions and writer serialization: https://sqlite.org/lang_transaction.html
- WAL/synchronous/secure_delete behavior: https://sqlite.org/pragma.html
- SQLite wrapper (pinned 0.43.0, bundled): https://docs.rs/rusqlite/0.43.0/rusqlite/struct.Connection.html and https://docs.rs/rusqlite/0.43.0/rusqlite/enum.TransactionBehavior.html
- OS-random worker identities: https://docs.rs/getrandom/0.4.3/getrandom/fn.fill.html
- Canonical payload fingerprints using RustCrypto SHA-256: https://docs.rs/sha2/0.11.1/sha2/
- Telegram update identity, acknowledgement offsets and server retention: https://core.telegram.org/bots/api#update and https://core.telegram.org/bots/api#getupdates
- Queue acknowledgement implementation checked for context: https://github.com/tdlib/telegram-bot-api/blob/master/telegram-bot-api/Client.cpp (do_get_updates). The public library contract remains conservative and does not rely on undocumented queue fallback behavior.
- Opaque cursor encoding: base64 0.23.1, already pinned in Cargo.lock; URL_SAFE_NO_PAD and Engine verified against installed registry source. Primary project: https://github.com/marshallpierce/rust-base64

Unknown effect handling is our conservative contract, not a Telegram idempotency guarantee. It intentionally treats connection and decode failures after dispatch as ambiguous.

- Bounded dispatcher task ownership and abort-on-drop: https://docs.rs/tokio/1.54.1/tokio/task/struct.JoinSet.html
- Fair cleanup/confirmation admission and close wakeups: https://docs.rs/tokio/1.54.1/tokio/sync/struct.Semaphore.html
- Cooperative timeout/cancellation limitations: Tokio 1.54.1 installed source src/time/timeout.rs, including the documented non-yielding future limitation; canonical reference https://docs.rs/tokio/1.54.1/tokio/time/fn.timeout.html (web fetch unavailable, local pinned source checked).
- Webhook secret header, accepted secret characters and non-2xx redelivery: https://core.telegram.org/bots/api#setwebhook
- Optional HTTP adapter: https://docs.rs/axum/0.8.9/axum/; Router<Request<B>> and the recommendation to use Hyper for connection configuration verified in the installed 0.8.9 source.
- Secret comparison uses the existing pinned subtle 2.6.1 implementation: https://docs.rs/subtle/2.6.1/subtle/trait.ConstantTimeEq.html
- Configured HTTP/1 parser and graceful connection shutdown: Hyper 1.12.0 installed source src/server/conn/http1.rs (Builder timer/header_read_timeout/max_headers/max_buf_size, Connection::graceful_shutdown), canonical https://docs.rs/hyper/1.12.0/hyper/server/conn/http1/struct.Builder.html

- Agent JSON framing: Tokio 1.54.1 installed source src/io/util/async_buf_read_ext.rs (fill_buf/consume, EOF and cancel safety), canonical https://docs.rs/tokio/1.54.1/tokio/io/trait.AsyncBufReadExt.html#method.fill_buf (web fetch unavailable; pinned source checked).
- Independent agent schema validation: jsonschema 0.61.2 installed source src/lib.rs validator_for / Validator::new; dev-only, default external HTTP/file resolving disabled. https://docs.rs/jsonschema/0.61.2/jsonschema/fn.validator_for.html (web fetch unavailable; pinned source checked).
- JSON Schema draft 2020-12 validation keywords, Unicode string length and non-anchored pattern semantics: https://json-schema.org/draft/2020-12/json-schema-validation

- MCP tool schemas, structured/text output, hints and tool errors: https://modelcontextprotocol.io/specification/2026-07-28/server/tools
- MCP stdio/current and legacy lifecycle: https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio
- Official Rust MCP SDK pinned to rmcp 3.10.0: https://docs.rs/rmcp/3.10.0/rmcp/ and https://github.com/modelcontextprotocol/rust-sdk . Exact APIs, default/optional features, SDK raw map parsing, bounded AsyncRwTransport, send/cancel/drain behavior and per-request metadata were checked in the downloaded crates.io 3.10.0 source, rather than assuming main-branch compatibility.

- MTProto backend probes: exact crates.io grammers-client/session/mtsender 0.12.0 sources and https://docs.rs/grammers-client/0.12.0/grammers_client/ . Configuration NoRetries/default AutoSleep, lazy SenderPool, unbounded RPC channel, child JoinSet ownership, Session state/serialization and MessageBoxes recovery were checked against that version. Current upstream is https://codeberg.org/Lonami/grammers (web access unavailable; downloaded release sources checked).
- Official TDLib commit 03220f6b3043c6fd7785fbf0901d63f19ad19974, CMake version 1.8.70, C JSON header and API schema: https://github.com/tdlib/td/tree/03220f6b3043c6fd7785fbf0901d63f19ad19974 and https://core.telegram.org/tdlib/getting-started . Native source build/C ABI evidence is recorded in ADR 0016, not inferred from docs.
- Native spike package URLs and published SHA-256 values: https://packages.msys2.org/packages/mingw-w64-ucrt-x86_64-openssl , https://packages.msys2.org/packages/mingw-w64-ucrt-x86_64-zlib , https://packages.msys2.org/packages/mingw-w64-ucrt-x86_64-gperf . Isolated target cache only; these packages are not production Rust dependencies.
- Authoritative update ordering/gaps and preservation of existing channel pts: https://core.telegram.org/api/updates and https://core.telegram.org/constructor/updateChannelTooLong . Used to identify the upstream unconditional channel-pts replacement, not to claim live compatibility.
- Maintained session AEAD prototype: https://docs.rs/chacha20poly1305/0.11.0/chacha20poly1305/ ; XChaCha20Poly1305 full-round 32-byte key/24-byte nonce, AAD and zeroize feature verified in pinned source and synthetic probes. No custom cryptographic primitive is introduced.
- Recursive ignored-field detection for strict encrypted snapshots: https://docs.rs/serde_ignored/0.1.14/serde_ignored/ . The callback records only a boolean; paths and values are never retained in errors. JSON end-of-input is checked separately.
- TL byte-string canonical lengths and zero padding: https://core.telegram.org/mtproto/serialize . The pinned grammers-tl-types 0.12.0 primitive decoder is patched separately; schema, generator, serializer and build script remain byte-identical to the published archive.
- MTProto control constructors, RPC/error envelopes, acknowledgements and whole-container acceptance: https://core.telegram.org/mtproto/service_messages . The separate grammers-mtproto 0.12.0 receive patch validates exact control bodies, bounded gzip expansion/depth and rollback on rejected containers; crypto and key derivation remain unchanged.
- MTProto authorization association, bot login and interactive code/SRP flow: https://core.telegram.org/api/auth , https://core.telegram.org/api/bots and https://core.telegram.org/api/srp . Exact TL shapes and upstream behavior were checked in the pinned grammers-client 0.12.0 auth.rs and grammers-crypto two_factor_auth.rs; the production wrapper owns identity, cancellation and persistence instead of copying upstream panic/retry behavior.
