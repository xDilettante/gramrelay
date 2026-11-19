# Scripted transport through the shared execution boundary

Accepted: 16 November 2026. Scope: T1 / test-kit.

The optional test-kit feature installs an in-memory script on BotBuilder. Bot keeps the same concrete public type. Reqwest still prepares JSON/multipart requests and streams request bodies; synthetic http::Response bodies convert into reqwest::Response and enter the production decoder. This avoids a second serializer, decoder or generic transport parameter throughout the public API. Missing scripts never fall back to HTTP.

Scripts, record counts and captured/queued body bytes are bounded. Claims and record reservations share one short mutex critical section; there is no lock across await. A response-stream lease keeps unfinished work visible until EOF, error or cancellation. No background task performs a delayed request or retry. Recorded body inspection is explicit; Debug omits fixtures and bodies, and records omit credential-bearing URLs.

This boundary tests application behavior, deadlines, retries and stream failures without sockets. It does not model DNS, TLS or HTTP parser behavior. Existing localhost wire tests remain required for those transport integration contracts. The client remains independent of database, CLI and MCP dependencies.
