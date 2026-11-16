# 0014: Trusted host bootstrap for standalone CLI and MCP

Status: accepted for A2. The same host module will supply A3; no second policy or
request execution path is introduced.

Host::load consumes a bounded absolute operator config and creates one principal,
policy, immutable bot profiles and journal-backed ToolSession. Only environment
variable names appear in config; token prefixes must match configured bot IDs.
Strict config objects reuse the executor's object visitor and reject unknown or
duplicate fields and positional arrays. Lists, source bytes and resource limits
are finite. Discovery does not invoke bootstrap or require credentials.

Static grant, approval expiry, resource, cursor and profile checks happen before
storage. Shared PolicyStore approval and cursor validators prevent drift from
execution semantics; journal-dependent ownership/position/reference checks remain
later. Profile update options reuse UpdateOptions validation and must match stored
feed policy. Startup never sends getMe or another RPC. On a dynamic bootstrap
failure the opened journal is explicitly closed before returning a static error.
Two static-approval omissions were found in review and fixed with observed
RED→GREEN regressions: denied chat/grant expiry and malformed cursor preflight.

Hosts may restore preissued files only by exact immutable owner/hash/size. Explicit
callback observations bind exact retained event cursors, not caller-attested chat
IDs. Missing positions never alias following rows; existing provenance, current
scope, original TTL and explicit clock trust still apply. Observation precedes
actual callback approval. These host controls are absent from tool schemas/data.

run/serve use the existing bounded adapters and shared session. EOF closes the
journal after serial work. Output failures do not replay writes or manufacture
NotSent; CLI errors retain keys and a later process can retrieve the committed
receipt. Process-level localhost tests cover every action, shared scope/quota,
restart/Unknown, approval binding, source context, restoration, dry-run, malformed
config and input, and an actual broken stdout pipe. No live Telegram calls occur.

OS-level access to config/environment/command line is host authority, not a tool
authentication boundary. Killing a process can leave an ambiguous effect; A1's
durable same-key recovery remains the contract. The full architecture goal and
A3/MTProto/release gates remain open.
