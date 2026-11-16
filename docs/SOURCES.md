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

Unknown effect handling is our conservative contract, not a Telegram idempotency guarantee. It intentionally treats connection and decode failures after dispatch as ambiguous.
