# Agent JSON interface and CLI (A2)

## Objective and trust boundary

Implement A2 from tasks/todo.md over the completed A1 executor. Preserve all eight
actions, stable order, current policy, quota, journal and Unknown semantics. A3
will adapt the same interface/catalog through official rmcp, not another executor.
No live Telegram calls, publication or remote push are part of verification.

Tool content is the existing version-1 Request only. The authenticated principal,
Executor, approvals and configuration are host capabilities. No tool can supply a
principal, token, transport address, path, grant, approval boolean or raw method.
Telegram content remains untrusted; discovery descriptions/examples are static.

## Interface and schemas

Public tools module under executor provides ToolSession, ToolResponse and a stable
catalog. A host explicitly creates a session with Executor/Principal. Host-only
approve registers at most 256 exact-request approvals through Executor::approve;
current grant revision/expiry is still checked at execution. No input can invoke
host approval. CLI/MCP share dispatch, response serialization and catalog.
execute_for(selected_action, request, mode) rejects a Request.call.tool mismatch
before admission. MCP binds selected_action from the registered server tool,
independently of input; JSON Schema/annotations never replace this runtime check.

Success: {schema_version:1,ok:true,result:<A1 ExecutionResult>}.
Dry-run: {schema_version:1,ok:true,dry_run:<A1 DryRun>}.
Error: {schema_version:1,ok:false,error:{code,message,
automatic_retry_allowed:false,effect_outcome?,operation_id?,next_action?}}.
Codes/messages are static and redacted; typed intentional results are distinct
from diagnostics. Validated write errors retain the original operation_id and
direct recovery to get_operation. Parse errors never echo arbitrary input.
ok means a result was obtained, not that a write succeeded: inspect operation
state/result availability. Unknown/Rejected/prepared receipts stay explicit.

Each catalog entry has fixed name/description/effect/replay advice, arguments
schema, complete Request input schema, response schema and success/error examples.
Use JSON Schema draft 2020-12, strict objects and canonical string identifiers.
Reject read operation_id fields (including null); require stable IDs for writes.
Schemas describe byte limits in prose where JSON Schema counts Unicode scalars;
runtime validation remains authoritative. Do not silently accept a broader tool
surface. Validate schemas with an independent validator against valid outputs,
boundary IDs, invalid injections and compatibility fixtures.

Checked-in schemas/v1 artifacts must match deterministic CLI generation. Schema
discovery never opens config/journal, reads tokens, consumes quota or sends RPC.

## CLI and host configuration

Optional cli feature builds tg-agent. Commands: tools, schema, run --config PATH,
serve --config PATH, with optional --dry-run on run/serve. Help is human-readable;
other stdout is JSON only. run accepts one bounded JSON document; serve accepts
NDJSON, one response per frame, processing serially. No banners/logs on stdout.

Trusted operator config is a bounded JSON file at an absolute path, version 1:
principal, absolute journal_path, profiles, optional approvals/file_restorations,
and explicit callback_clock_policy. Profile fields: name, canonical bot_id string,
token_env, grant, optional operator api_url/allow_http_loopback. Tokens come only
from environment variables named in config, never config literals/tool input.
Grants explicitly select actions/chats/files/global scope, expiry, approvals and
a stable bounded budget identity/limits/window. Defaults deny; malformed/unknown
fields, versions, duplicates and excessive collection sizes fail closed before
execution. Host config/paths/environment are trusted operator controls; agents
must not be allowed to modify them or the process command line.

File restorations reference preissued immutable handles and absolute host paths;
read at most one bounded file at a time and restore only matching owner/hash/size.
Registration remains the trusted Rust host API. CLI source reads add at most one
bounded input buffer outside executor residency, like other host-owned inputs.
Explicit config approvals contain exact typed Request and absolute expiry, then
call the host approval API. No automatic consent is inferred from tool requests.
Standalone callback reads require the explicitly configured TrustedWallClock
mode when original monotonic provenance belongs to another journal lifetime;
default JournalMonotonic remains fail-closed.

Input document/frame limit is Request::MAX_JSON_BYTES before dispatch. Bounded
framing never read_until-allocates an arbitrary line. Oversize closes the stream
after one safe error; malformed bounded frames receive an error and serve keeps
reading. Output defaults to 8 MiB, configurable for embedded hosts from 1 KiB to
64 MiB. Enforce the serialization cap before output; oversized post-effect output
reports Unknown/recovery for a validated write, never NotSent. IO failure never
causes another execution or success claim. EOF drains serial work and closes the
journal. OS process termination can leave an ambiguous write; recover the same key.
run exits 0 for a returned result, 1 for a tool error, 2 for usage/config/framing/IO
failure. serve exits 0 after EOF even if individual frames failed.

## Structure and verification

tools/{catalog,session} holds shared contracts; cli holds framing/config/entrypoint;
src/bin/tg-agent.rs is a thin shell. No new transport or protocol implementation.
Test library dispatch with the existing in-memory MockTransport and executable
stdin/stdout with localhost synthetic HTTP fixtures. Check scope/revocation,
approval, durable same-key/Unknown recovery, local reads, file restoration,
framing/size/IO failures, redacted diagnostics and discovery without credentials.

Commands: cargo test --locked --workspace --all-targets --all-features;
cargo test --locked --workspace --doc --all-features;
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings;
cargo fmt --all -- --check; cargo audit. Also check executor-only/storage-only
builds and deterministic schema artifacts. A2 stays open until the executable
uses real host policy/executor and all acceptance cases are verified.

## Implementation increments

1. Shared ToolSession/envelope/catalog, independently validated schemas, bounded
   embedded run_json/serve_jsonl and executable tools/schema discovery. Keep A2
   unchecked while standalone host execution is absent.
2. Bounded trusted config loading, environment-only credentials, scoped profiles,
   immutable file restoration and explicit approvals; wire standalone run/serve.
3. Executable-level localhost execution/restart/approval/IO regressions, complete
   docs and full gates. Only then mark A2 complete; A3 still remains separate.
