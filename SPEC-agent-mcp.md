# MCP stdio adapter (A3)

## Boundary and interface

Use the official Rust SDK `rmcp` 3.10.0, with no HTTP listener, alternate
executor, tool macros or model-supplied authority. The optional `mcp` feature
adds `tg-agent mcp --config ABSOLUTE_PATH [--dry-run]` and a reusable adapter
over a trusted `ToolSession`. Startup failures write only a static code to stderr;
stdout is exclusively MCP JSON-RPC. Host shutdown always closes its journal.

Expose exactly the existing eight tool names in catalog order, with identical
input/output schemas and annotations. MCP `arguments` is the full version-1
Request (including `call.tool`); bind the separately selected MCP name through
`ToolSession::execute_for` before admission. Reject unknown names and pagination
cursors with static protocol errors. Invalid tool input and execution denials
are structured tool errors. No tool may issue approvals, register profiles or
read arbitrary files. Tokens, addresses and paths remain protected host config.

Return the shared ToolResponse envelope both as structuredContent and as one
JSON text block. `isError` follows ToolResponse::is_error; a receipt is a result,
so clients must inspect operation state even when `ok` is true. Unknown is never
permission to replay. Telegram JSON/text is untrusted data, never instructions.
The server advertises tools only, without list-change notifications or tasks.
SDK lifecycle handles current protocol and legacy initialization negotiation.

## Limits and lifetime

Bound each incoming JSON-RPC line to 128 KiB using the SDK transport; the shared
Request parser retains a 64 KiB limit on the normalized arguments object. Unlike
CLI raw bytes, MCP objects are decoded by the SDK before shared dispatch. A
bounded raw-frame guard rejects duplicate decoded member names, including
escaped keys and nested objects, before SDK map normalization; it closes the
connection on duplicates or oversized frames. Syntax/protocol handling remains
the SDK's job. Keep at most 32 requests pending through
response flush, reject duplicate active JSON-RPC IDs, and close on overload.
Cancelled IDs retain a slot until reconnection, including responses that were
already being sent. This prevents an old SDK response from being attributed to
a reused ID. After at most 32 cancellations a client must reconnect.
This bound includes metadata requests and responses waiting behind slow stdout.
Shared output serialization defaults to 8 MiB before duplicating JSON into the
MCP structured and text representations. SDK protocol framing adds finite
overhead; it does not change the shared tool envelope limit.
An embedded trusted host may choose OutputLimit within 1 KiB..64 MiB. All output
writes/flushes, including SDK-generated parse errors, have a five-second idle
deadline; normal response sends also have a five-second absolute deadline.

Client cancellation drops the executor future. The durable journal determines
the operation outcome: cancellation after admission can leave Unknown and must
not trigger any automatic retry. Connection close cancels unfinished handlers;
wait for adapter calls to leave before closing host state. Lost output is
recovered only with get_operation and the original operation ID.

## Acceptance

Use real SDK protocol integration plus executable stdio tests, localhost or
synthetic SDK transport only. Cover deterministic discovery/schema equivalence,
current and legacy lifecycle, all eight actions, wrong selected tool, invalid
input, scope/approval denial, dry-run, same-key replay, Unknown recovery across
restart, cancellation, EOF/broken output, bounded input/output and overload.
Document launch and recovery with runnable examples. Read-only evaluation cases
must be independently checkable against a deterministic offline fixture; do not
claim the later Q1 agent evaluation or platform gates from these checks.

## Sources

- https://docs.rs/rmcp/3.10.0/rmcp/
- https://modelcontextprotocol.io/specification/2026-07-28/server/tools
- https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio

The downloaded crates.io source for rmcp 3.10.0 is authoritative for exact Rust
APIs and dependency features; tests pin wire compatibility rather than assuming
that the repository main branch matches the released crate.
