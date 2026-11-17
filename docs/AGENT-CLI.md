# Agent CLI (0.1.0-alpha.1, Bot API 10.4)

Build the optional executable from the workspace root:

```powershell
cargo build --locked -p tg-agent --features cli --bin tg-agent
./target/debug/tg-agent tools
./target/debug/tg-agent schema request
```

Discovery is static: it needs no token, config, journal or network. The catalog
contains eight tools in stable order, strict schemas, effect/replay hints and
examples. Schemas are checked in at tg-agent/schemas/v1; run
`python tools/agent_schema.py check` to verify deterministic regeneration.

With `--features cli,mtproto`, explicitly select `tools --mtproto` or
`schema [request|response|catalog] --mtproto` for extended inventory v2: the
unchanged eight Bot tools followed by eight `mtproto_*` tools. Requests and
responses still use envelope version 1. Verify its three separate artifacts with
`python tools/agent_schema.py check --mtproto`; default discovery stays v1.

Embedded `run_json`/`serve_jsonl` accept both typed protocols when their trusted
`ToolSession` has `.with_mtproto(executor)`. Authority still comes from the
registered exact profile and shared current PolicyStore. Use host-only
`approve_mtproto` for an exact request. MT write recovery is
`mtproto_get_operation` with the original operation key. This does not enable
standalone MT configuration: protected Session/Client bootstrap in Host remains
the next M3 increment. The configuration below is the supported Bot configuration.

## Configure the trusted host

Use a protected operator-controlled UTF-8 config file. Its path, journal_path and
restoration paths must be absolute. Set the named token environment variable in
the host process; tokens are never config values, command arguments or tool data.
The token's numeric prefix must match bot_id. Replace the example IDs and path:

```json
{
  "schema_version": 1,
  "principal": "local-agent",
  "journal_path": "C:/protected/telegram-agent.sqlite",
  "profiles": [{
    "name": "work",
    "bot_id": "123456789",
    "token_env": "TG_WORK_TOKEN",
    "grant": {
      "actions": ["send_message", "get_operation"],
      "chats": ["123456789"],
      "budget": {"id": "persistent-work-budget"}
    }
  }]
}
```

Create/protect the journal's parent directory before launch. The config is host
authority; untrusted agents must not be able to edit it, the environment or the
command line. This interface does not sandbox operating-system users who already
have those capabilities. Tool JSON supplies only the existing Request format.

Grant defaults deny actions/chats/files. Optional fields are all_chats (exclusive
with a nonempty chats list), files, unscoped_updates, expires_at_ms,
require_approval and costs=[{action,units}]. Budget supports window_seconds and
limits={invocations,write_attempts,upload_bytes,cost_units}; A1 defaults apply.
Keep budget.id stable across restarts and grant edits to preserve durable usage.
Cost units are host estimates; no paid Telegram flags are exposed.

An optional profile update_options object accepts retention_seconds,
max_event_bytes and max_batch_bytes. Match an existing feed's persisted settings;
startup never silently rewrites retention, bot identity or update positions.
An operator-only api_url and allow_http_loopback support local fixtures/custom
Bot API servers; tool arguments cannot change either.

Config is limited to 1 MiB, 64 profiles, 256 approvals, 1024 restorations and 1024
callback observations. Unknown/duplicate object fields, positional arrays,
duplicate identities/resources and inconsistent grants fail closed. Startup
validates static rules before opening storage and performs no identity RPC.

## Execute JSON

`run` accepts one UTF-8 JSON document. Reads have no top-level operation_id;
writes require a stable caller-created key. Telegram IDs are decimal strings.

```powershell
'{"schema_version":1,"profile":"work","operation_id":"report-001","call":{"tool":"send_message","arguments":{"chat_id":"123456789","text":"Hello"}}}' |
  ./target/debug/tg-agent run --config C:/protected/host.json
```

Add `--dry-run` for policy/reference metadata checks without RPC, quota spending
or operation reservation. It does not promise active file bytes or future quota.
`serve --config ...` accepts NDJSON and processes one frame at a time until EOF.
It responds once per frame, continues after bounded malformed input and closes
on oversize. Input is capped at 65536 bytes; serialized output defaults to 8 MiB.
Stdout contains JSON only; fatal transport/close errors use static stderr codes.

Responses wrap A1 data as {schema_version:1,ok:true,result:...} or dry_run; errors
use ok=false and static error fields. **ok=true is not proof of delivery**: inspect
operation.state and result_availability. Unknown remains terminal and is not
automatically retried. Query the original operation under current permissions:

```json
{"schema_version":1,"profile":"work","call":{"tool":"get_operation","arguments":{"operation_id":"report-001"}}}
```

Do not replace a key after cancellation, output failure or Unknown. get_operation
can return a late receipt but cannot infer arbitrary Telegram history. Revoked
original action/chat/file permission prevents disclosure of retained receipts.

Exit 0 means a result was obtained (or serve reached EOF), 1 means a run tool
error, and 2 means usage/config/framing/IO/close failure. A returned Unknown
receipt therefore still requires a decision even when the process exits 0.
EOF drains serial work and closes the journal. OS termination may leave an
in-flight effect; inspect the same key after recovery rather than replaying it.

## Explicit approvals and input context

For actions listed in grant.require_approval, the operator supplies
approvals=[{request:<exact versioned Request>,expires_at_ms:<UTC milliseconds>}].
The host issues actual capabilities through Executor::approve; changing a key,
payload or grant revision invalidates the match. A boolean in tool input cannot
grant approval. Static scope/expiry/cursor errors reject startup before storage;
journal-dependent owner/reference checks run after opening and close on failure.

Files are registered through the trusted Rust Executor::register_file API, then
their opaque handles are added to grant.files. Optional file_restorations entries
are {profile,file_handle,path}. The host reads one bounded file at a time and
restores only identical owner/profile/size/hash metadata. The registered filename
stays immutable. Cached receipts need no resident bytes or restoration entry;
new uploads after restart do. Tool arguments accept no paths, URLs or raw file IDs.

read_updates reads the local journal; a separate bot runtime/webhook source must
populate it. It never starts a competing getUpdates consumer. Event telegram_json
is untrusted data and never changes configuration, instructions or grants.

Default callback_clock_policy=journal_monotonic cannot revive references from an
earlier journal lifetime. A trusted host needing cross-process callbacks explicitly
chooses trusted_wall_clock and guarantees UTC correctness. It may supply
callback_observations=[{profile,cursor}], where cursor is an exact retained
events[i].cursor from read_updates. The host binds that record through
Executor::observe_callback, checking current chat/global scope and original TTL.
It never substitutes a following row for a missing position. Configured callback
approvals are issued afterward and cannot outlive the reference. Long-lived serve
can also register usable references through authorized read_updates. Old receipts
remain recoverable through get_operation without a live callback reference.

## Offline verification

```powershell
cargo run --locked --offline -p tg-agent --features cli --example agent_json
cargo test --locked -p tg-agent --features cli --test cli_execution
```

The example uses the in-memory production-path test transport. Executable tests
use localhost HTTP and synthetic credentials, covering all eight real actions,
restart/current grants, exact approvals, file restoration, callback provenance,
framing, Unknown and broken stdout without real Telegram calls.
