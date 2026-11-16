# ADR 0015: official MCP stdio over the shared executor

Status: accepted, 2026-11-16.

Use optional rmcp 3.10.0 server support with the fixed shared catalog and full
versioned Request arguments. One ToolSession owns host authority across Rust,
CLI and MCP. No duplicate executor, alternate approval protocol or tool macros
are introduced. MCP structured and JSON text results use the existing envelope;
receipts remain explicit even when an operation did not succeed.

The SDK owns JSON-RPC, protocol negotiation and cancellation delivery. A bounded
raw-line guard rejects duplicate decoded JSON keys before SDK maps discard
lexical information. The shared 64 KiB Request limit applies to normalized MCP
arguments; the raw envelope has a separate 128 KiB bound. This differs explicitly
from the CLI's raw-document byte accounting.

Keep 32 request slots through response flush, including cancelled IDs until the
connection ends. Reconnect after cancellation saturation. Releasing cancelled
IDs early allows an old SDK response to remove a newer request's token or lets
responses accumulate behind blocked stdout. A guarded AsyncWrite also bounds
SDK-owned protocol-error flushes, which bypass Transport::send. Host close waits
for executor futures to leave; standalone runtime shutdown bounds blocking
standard-stream workers after journal close.

Regression tests cover actual SDK/client and executable stdio interactions,
both protocol eras, scopes/approvals, all eight actions, files/callbacks,
same-key/Unknown recovery, cancellation/EOF, raw duplicates, limits and output
failure. A synthetic archive and ten read-only questions prepare ergonomic
evaluation; they are not evidence of the later LLM acceptance score.
