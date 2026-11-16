# Authorized local reads and lossless untrusted events

Status: accepted for A1 get_me/get_chat/get_operation/read_updates.

The same executor now admits all four reads through current policy and durable
invocation/cost quota. Remote reads use one SDK attempt and a shared immutable
profile identity cache. get_chat verifies the returned ID. Local reads never
start a competing Telegram update source or require a fresh identity RPC.

get_operation requires its own action plus the persisted original action and
bot/chat/file/global scope, checked for the current principal. It cannot infer
scope from cached JSON or new arguments. Approval on the original write does not
need to be renewed just to inspect its receipt; approval on get_operation itself
still applies. Executor dry-run/approval reads immutable ownership/scope metadata
only, with no payload disclosure, operation refresh, quota charge or RPC. Cursor
dry-run similarly validates position metadata without consuming events.

read_updates scans at most 100 entries within a configured raw JSON byte budget
(1 MiB by default). Payload length is checked before allocating its Rust buffer;
an oversized first row fails explicitly. Consuming pages moves their update
payloads rather than cloning them. The cursor stops at the last considered row,
including filtered rows, and never skips unread rows after reaching output limit.
Every call filters against current grants; cursors are positions, not authority.

Primary scope is reviewed separately from dispatch ordering hints. Forwarded or
nested chat IDs cannot change it. Unknown top-level data, guest/widget/account/
inline contexts and mixed message+inline callbacks require explicit unscoped
authority. Recognizable ordinary chat scopes remain additional constraints;
the global flag cannot expose a forbidden or invalid ordinary chat. Ambiguous
callbacks issue no action reference metadata.

The wire envelope has canonical string target IDs and a full telegram_json
string marked untrusted. Rust callers retain a typed Update accessor. This
preserves nested/future numeric IDs without forcing JavaScript rounding. Unknown
fields inside an authorized event remain data, never grants or tool definitions.
Bot/chat/operation projections are allowlisted and diagnostics redact contents.

Seventeen focused tests cover scopes, current revocation, owner/original-action
checks, dry-run non-mutation, approval, restart quotas, cursor limits/retention,
byte budgets, unknown/ambiguous events and extreme numeric precision. Fresh
review found missing ID projections and mixed callback scope; independent RED
tests also caught expired dry-run cursors and unscoped chat bypass, all fixed.
