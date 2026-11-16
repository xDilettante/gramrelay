# Authorized shared executor (A1)

## Objective and trust boundary

Implement the agent-runtime executor in docs/ARCHITECTURE.md. Rust, CLI JSON and MCP use the same typed calls, current authorization, quotas, operation journal and safe results. A1 is incomplete until all eight initial actions below are integrated. No live Telegram actions, publication or remote push are needed for development; tests use the public test-kit and localhost fixtures.

The host registers immutable bot profiles, authenticated principals, grants, approvals and file snapshots. Tool arguments cannot choose a principal, token, API URL, local path, grant, budget or approval. Principal/approval host capabilities are not deserializable from tool arguments. Telegram content remains untrusted data, never policy/configuration or a tool description. There is no raw_call or dynamic Bot API dispatch bypass.

The executor feature is optional and does not imply the bot/webhook runtime. It uses the existing tg client and SQLite worker; CLI/MCP features are added by A2/A3. Shared canonical numeric-ID strings avoid precision loss in JSON/JavaScript. Reject aliases, noncanonical forms, zero chat/message IDs, unknown fields/tools, overlong strings, invalid versions and non-write operation_id fields. Bound input bytes/depth/nodes before use. Incoming/outgoing Debug/error chains omit payloads, identifiers, credentials and file paths; intentional typed output is separate from diagnostics.

## Initial action surface

Stable order: get_me, get_chat, read_updates, send_message, edit_message_text, answer_callback_query, get_operation, send_document.

| Action | Scope and result | Effect |
|---|---|---|
| get_me | Current profile grant, verified bot ID; minimal bot projection | Read |
| get_chat | Numeric authorized chat ID; response target must match, minimal chat projection | Read |
| read_updates | Local feed only, current per-event chat/global permission; bounded scan/cursor/limit | Local read |
| send_message | Numeric chat ID, plain text, optional thread/notification controls; minimal message receipt | Write |
| edit_message_text | Numeric chat/message IDs, plain text; no unbound inline ID | Write |
| answer_callback_query | Reference registered for this principal/profile by authorized read/host context, current chat/global scope and expiry | Write |
| get_operation | Current action permission, owner and persisted original resource scope; minimal state/receipt | Local read |
| send_document | Numeric chat ID and host-registered immutable file handle; caption and minimal message receipt | Write |

Paid broadcast, payments, bulk/admin/delete methods, arbitrary URLs, raw Telegram file IDs and arbitrary inline/business targets are not tool arguments in this initial reviewed surface. Full Bot API remains available through the separate Rust SDK. Every response projection is allowlisted; remote descriptions/unknown fields do not enter safe receipts. Authorized read_updates preserves full update content and identifies it as untrusted Telegram data.

## Host policy and admission

One current grant per (principal, profile) avoids ambiguous unions/choosing another quota. It binds the exact expected bot ID, action set, chat set or explicit all-chats, file-handle set, explicit unscoped-update permission, expiry, confirmation requirements and budget identity/limits. Defaults deny access. Bounds limit registered profiles/grants/references/files. Host replacement/revocation is atomic; changing a grant does not silently reset durable usage.

Check current grants before identity RPC, execution admission and result disclosure. The profile is permanently bound by its UpdateFeed to a numeric bot identity; first permitted remote use verifies getMe and caches that immutable client's identity. Profile replacement is not accepted through tool arguments. Local dry-run validates input, policy and approval requirements without network dispatch or quota/operation mutations; it does not promise Telegram success.

Revocation affects new admissions and cached/read-result disclosure. Work already admitted/dispatched may finish; revocation cannot undo remote effects. Recheck after async preparation and immediately before execution; a locally denied/not-started attempt must be recorded NotSent, not left executable or misreported as a remote refusal. An approval is an explicit host capability bound to principal/profile, canonical fingerprint of the entire versioned request (including profile, action, arguments and operation_id), issuing policy store, grant revision and expiry. It is never a boolean supplied by a model. Ordinary permanent grants need no per-call approval unless the host requires it.

read_updates scans at most one bounded journal page (up to 100, within its byte budget) and returns at most the requested authorized events. Advance only over actually considered rows, so reaching an output limit never skips later unread authorized events. Existing versioned feed cursors remain positions, not grants; every call rechecks scope, including unknown/unscoped events. Scoped actors may observe feed position/activity metadata but never forbidden event contents. The call never starts getUpdates/polling and never claims arbitrary Telegram history.

Callbacks are ephemeral references bound to trusted observed input and caller/profile/scope, not self-attested chat_id arguments. Registration uses the original event receipt time; rereading cannot refresh an old callback indefinitely. The canonical (principal, profile, callback_query_id) reference is reused when the same observed event is reread; aliases cannot create independent admission rights. One callback reference cannot be rebound to a different operation after an admitted attempt; NotSent can retry the same key, while Unknown cannot be bypassed with a new key. References and their metadata are bounded/expired explicitly. Current get_operation access can inspect an authorized retained receipt independently of whether an old callback can still be answered.

## Durable execution and quota contract

Use J1 prepare/begin/complete with its immutable (profile, operation_id), principal, action and canonical payload hash. All writes require a stable caller-created operation_id. Bind minimal authorization scope metadata transactionally to the row; never reconstruct scope from arbitrary cached JSON or fresh tool claims. Legacy unscoped journal rows fail closed at the executor boundary. Store bot/action/chat/file/reference scope, not message text or credentials. Current scope/owner checks precede cached-result or get_operation disclosure. Revoking chat/file access prevents disclosure of its old results.

Schema v4 adds reviewed bounded budget and operation-scope tables to v3 without changing existing operation/update/consumer data. Validate old/new DDL and migrate in one transaction. Budget and scope operations use the same native DB worker and parameterized SQL. Limit/count/overflow checks happen under BEGIN IMMEDIATE with time sampled after acquiring the write lock.

Budgets are per stable host grant identity and principal/profile. Fixed-window durable counters bound authorized invocations, admitted write attempts, upload bytes and host-defined integer cost units. These cost units are policy estimates, not a promise about an external bill; paid Telegram options are not exposed. Cached write responses consume invocation quota but do not spend another write/upload/cost admission. Each new executable attempt, including a retry after confirmed NotSent, is charged conservatively; ambiguous failures are never refunded/replayed. Reads charge invocation/read cost. Charge write quota atomically with begin. Duplicates that lose the claim do not obtain another attempt.

Restarts and grant revision/revocation do not reset counters. Window advancement is monotonic per ledger row; backward clock movement cannot reopen a previously spent bucket. Changing window duration for an existing budget identity fails closed; an explicit new budget identity is a host decision. Updated caps apply to retained usage. Ledger capacity rejects new identities rather than evicting live usage. Independent callers/connections cannot exceed a shared cap. Default host limits are finite and configurable; aggregate executor concurrency/upload memory is bounded separately from per-grant quotas.

Prevalidate local params before reserving an operation key. Only after an authorized, quota-checked durable begin may a lazy SDK request be polled. Use one bounded attempt through the real SDK transport; no implicit retries/migrations/paid flags. Keep network budget plus receipt margin within the journal lease. Confirmation uses fair DB admission so its own concurrent/cancelled work cannot manufacture Capacity. SDK success is projected before storage, and an unexpected response target is never disclosed as authorized success. Failed/uncertain receipt persistence after dispatch is Unknown to the executor, never NotSent. Cancellation after begin is recoverable through the same key; it never creates a new one.

## File boundary

Host registration creates an opaque nonreused handle bound to principal/profile, immutable SHA-256/size/filename metadata and expiry. Tool input accepts only that handle, not a path/URL/file_id. Bounded immutable snapshots prevent TOCTOU between approval/hash and upload. Metadata remains durable for operation/result scope checks; active snapshot bytes may be unavailable after restart, which prevents a new upload rather than changing content or replaying an old action. A host can restore matching bytes for a retained handle only after hash/owner checks. Expiry prevents new upload; current grants still control scoped receipt disclosure.

Include handle and immutable content metadata in the canonical operation fingerprint. Bound per-file/total snapshot bytes, registered handles and active upload bytes. Filename validation precedes multipart preparation. Captions are plain text. A cached result's missing/expired body never permits reexecution. No binary/file/token material or raw local path enters operation rows, tool schemas or errors.

## Implementation increments and verification

1. executor/{policy,types}.rs: stable action inventory, host-only principal/grant/approval capabilities, resource checks, strict typed arguments and pure dry-run. Test deny by default, bot/profile/action/chat/file/global scopes, expiry/revocation/approval binding, argument privilege injection and JSON precision/bounds.
2. journal v4 + executor write slice: persistent scope/quota admission, verified profile, send_message and minimal receipts. Test concurrent claims/quotas, restart, cache/current-scope/owner checks, changed payload, terminal Unknown/Rejected/success, NotSent, cancellation and receipt failure.
3. read/get_operation + callback/edit slice: bounded authorized local updates, context references and current result filtering, safe projections/target checks. No independent network update consumer.
4. files/send_document: registered immutable snapshots, durable metadata, strict handles, restoration/expiry, memory caps and scope/hash/TOCTOU abuse cases. Complete all eight actions, safe versioned output types, offline examples and focused adversarial review.

Commands: cargo test --locked --workspace --all-targets --all-features; cargo test --locked --workspace --doc --all-features; cargo fmt --all -- --check; cargo clippy --locked --workspace --all-targets --all-features -- -D warnings; cargo audit. Run schema/migration and isolated feature checks appropriate to each increment. A1 completion requires all actions and abuse/recovery invariants, not just a passing send-message slice. A2/A3 publish the same typed schemas/semantics through CLI and official rmcp.

Current implementation: increments 1 and 2 are integrated. execute supports
send_message; the other seven typed actions explicitly return UnsupportedAction
until increments 3/4. Policy-only dry-run remains local and reference lookup
remains ContextRequired. Schema v4 freezes authorization.sql; later durable file
registration metadata will require a subsequent reviewed migration, not an edit
of v4. The current ledger capacity is configurable (4096 by default). Default
executor concurrency/RPC/receipt budgets are 16/30 seconds/5 seconds. Tests cover
ten policy, eight admission/storage, eleven end-to-end send/recovery and five
migration cases. The offline authorized_send example makes one identity read and
one send for two invocations. Full A1 and the architecture goal remain open.
