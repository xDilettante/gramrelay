# MTProto agent tool integration (M3)

## Objective and contract

Connect the eight reviewed MTProto Executor actions to the existing shared
ToolSession, bounded CLI JSON and official-SDK MCP adapters. Keep the Bot API v1
catalog and its three checked artifacts byte-identical. Explicit extended
discovery (`tools --mtproto`, `schema [request|response|catalog] --mtproto`) returns
catalog/bundle version 2: the original eight tools followed by the eight
`mtproto_*` names in Action::ALL order. Individual request/result/error envelopes
remain version 1; catalog version describes its inventory, not a new request.

ToolRequest is a strict sum of the existing Bot and MT requests. Parse the raw
bounded bytes with their original typed visitors; do not normalize through Value
or accept duplicate/unknown fields, positional objects or authority fields.
MCP retains A3's reviewed boundary: reject duplicate decoded names in the raw
128-KiB frame before the SDK; the shared 64-KiB arguments limit applies to the
SDK-normalized object. CLI/ToolRequest enforce their cap on original input bytes.
ToolSession::new remains Bot-only. A trusted host explicitly attaches a cloned
MT Executor using with_mtproto. Embedded adapters advertise that session's fixed
catalog; a request cannot enable a provider. The selected MCP name must match
the complete request's tool independently before any executor work.

Reuse typed execution results, policy/approval checks and the existing output
cap. Approved requests share one bounded 256-entry host-only capability store
with protocol-separated keys; a Bot approval cannot authorize an MT request.
Validated MT write delivery/cancellation errors retain the original operation
key, outcome Unknown and next_action=mtproto_get_operation. Diagnostics never
reflect input, credentials, raw TL or remote descriptions. Same-key replay and
Unknown behavior remain entirely the Executor's responsibility.

The extended schemas describe actual tagged peers, boolean defaults, int32
message targets, safe projected update variants and minimal operation receipts.
Use strict objects, canonical decimal strings and draft 2020-12. Unicode length
is advisory in schemas; MT runtime enforces UTF-16 limits. Independent validation
must cover actual success/dry-run/error output, tagged-ID boundaries, unsupported
authority injection and all event variants, not only schema-authored examples.

## Structure and implementation order

1. tools/request.rs and tools/session.rs: typed sum, host attachment, exact
   approvals, dispatch and recovery context. Existing Bot Rust entry points stay.
2. tools/catalog/mtproto.rs: extended schemas/catalog; codec and CLI/MCP use the
   common request/recovery path. Add checked schemas/v2 through agent_schema.py
   with explicit --mtproto, preserving its default v1 behavior.
3. Trusted Host MT bootstrap and standalone executable fixtures (next increment):
   versioned config with exact account/session binding, environment-only secrets,
   protected paths, shared policy, explicit observations/restoration/approvals,
   no startup RPC, and joined Executor → Client → Journal shutdown. Tool JSON
   never supplies auth/login/raw/session/path authority. M3 remains open until
   this increment and real executable acceptance are complete.

## Verification and boundaries

Use Rust tests with synthetic encrypted Sessions, actual TL codecs and socket-free
test-kit. Exercise all eight names through shared dispatch and embedded adapters,
both protocol catalogs, exact approval/revocation, Unknown/one-dispatch recovery,
callback/file output, cancelled/broken/oversized delivery and raw parsing.
No live credentials or Telegram actions, publishing, push or new dependencies.
Do not create another transport/executor or copy its authorization logic.

Commands (workspace root):

```
cargo test --locked -p tg-agent --all-features --test mtproto_tools
cargo test --locked --workspace --all-features --all-targets
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --all-features --doc
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cargo fmt --all -- --check
python tools/agent_schema.py check
python tools/agent_schema.py check --mtproto
python -m unittest discover -s tools/tests -v
python tools/schema.py check
python tools/verify_grammers.py check
cargo audit
```

Always validate current typed intent and selected tool; preserve owned MT jobs
after caller cancellation and safe final disclosures. Config/SDK lifetimes remain
trusted host responsibilities until the explicit bootstrap increment. Record
actual evidence and remaining release gates in tasks/todo.md before delivery.

## Shared adapter outcome

The first two increments are implemented: typed ToolRequest, explicit provider
attachment and session-selected catalog, shared bounded approval/dispatch/output
paths, embedded CLI and official MCP, explicit credential-free extended discovery
and three separate reproducible schemas/v2 artifacts. The original schemas/v1
and dependency lock were byte-identical at this checkpoint. Standalone MT host
bootstrap followed in SPEC-mtproto-agent-host.md and completes increment 3;
the earlier shared-adapter evidence below remains historical.

Thirteen additional test cases cover all eight actual TL actions through CLI
and MCP, same-key cached writes after file discard, all eight actual dry runs,
exact approvals/revision refusal, actual denied/invalid/mismatched/oversized
outputs validated independently against schemas, strict typed parsing and peer
ranges, all twelve Event variants after real Serde serialization, broken output,
timeout Unknown without replay, and explicit notification cancellation while the
connection remains open through confirmation. A separate EOF branch keeps stdout
readable, joins the adapter and then releases the still-owned native write.

The initial contract test failed before the API existed. Isolated CLI without MT
then exposed a missing type annotation; its observed E0282 and passing CLI-only
check plus both executable discovery tests prove the correction. Adversarial
review identified missing actual-error schema validation and conflated EOF/
notification evidence; both checks were strengthened and focused re-review is
clear. Closing both halves of the test pipe correctly produced a transport error;
the clean-EOF fixture now models stdin EOF while keeping stdout readable.
Contract, schema and execution reviews have no unresolved findings. Cross-model
review was skipped in this autonomous continuation.

Windows/Rust 1.98.0: 653 all-feature / 389 default workspace tests, 16 doctests,
all-feature Clippy/fmt, isolated storage/executor/MT/MCP checks and CLI-only
all-target Clippy pass. Twenty-five Python tests, 193-method/425-type schema,
four pinned SDK verifiers and both agent schema inventories pass. Fresh audit
scans the unchanged 363 dependencies with the known informational chrono warning;
Q3 must remove it before release. M3 host bootstrap and all Q1–Q4 gates remain.
