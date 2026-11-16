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
- Cancellable synthetic chunk streams: https://docs.rs/futures-util/0.3.32/futures_util/stream/fn.unfold.html

Unknown effect handling is our conservative contract, not a Telegram idempotency guarantee. It intentionally treats connection and decode failures after dispatch as ambiguous.
