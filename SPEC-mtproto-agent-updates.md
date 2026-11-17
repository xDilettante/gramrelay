# MTProto agent update journal (M3)

Extends SPEC-mtproto-agent.md and SPEC-mtproto-updates.md. Implement real
M2 receiver → safe projection → journal commit → M2 whole-batch ACK. Tests use
the opt-in RPC fixture with real TL codecs and encrypted local sessions; no
live credentials, existing private sessions or Telegram connections.

## Source and bounded deduplication

M2 exposes non-authorizing metadata: expected account, a domain-hashed persistent
storage identity, and the existing monotonic update ordinal on batches/gaps.
These getters are not ACK authority. The source hash survives encrypted reopen;
another store differs. An explicit test-kit feature enables a socket-free Client
factory using the already reviewed owned SDK fixture. Production construction
is unchanged and agent data cannot select this factory.

Journal schema v7 adds separate MTProto feed/event tables referencing exact
account_profiles. Preserve and verify all v1–v6 state before migration. Bot and
MTProto share global feed/row/payload budgets; quotas do not silently double.
Each feed has immutable retention/event/read limits and one fenced source lease.
Native worker close releases only its own source. Source proofs are private,
bind account/profile/worker/revision and use UTC plus a local monotonic deadline.

Keep one fixed-size source watermark (source hash, u64 ordinal, ID, fingerprint,
record kind and original event range), rather than permanent per-batch rows.
Same ordinal/ID/content replays without inserting, refreshing time or callback
provenance, even after event expiry. Lower ordinals, conflicting content or a
different source are explicit refusals. Higher ordinals append atomically.
Never infer remote ACK from a later ordinal: local credential discard can also
advance the ordinal. Explicit host source replacement records a discontinuity,
rotates the source lease and preserves old events/cursors. It cannot be a tool
argument or automatic recovery; the host joins the prior Client first.

## Safe records and storage

Project actual TL updates into a versioned allowlist: new/edited regular,
service and empty messages; scoped/unscoped deletes and read positions; bot
callbacks. Preserve plain text and callback bytes as untrusted content, typed
primary peer and canonical ID strings. Never use forwarded/sender/nested peers
as scope. Drop raw user/chat entity graphs, access hashes, file references,
inline message capabilities and other reusable metadata from agent output.
Other constructors yield explicit unsupported markers requiring unscoped access,
not a guessed empty successful event. Malformed known data refuses the whole
batch before ACK. Baseline, empty checkpoint, resync preparation and gap are
explicit control records. A resync record alone does not claim checkpoint ACK.

Store private callback query metadata separately from public projection, with
original receipt time and same-journal provenance. Reuse the bounded monotonic
receipt tracker only for freshly committed callbacks; duplicates/restarts never
refresh it. This metadata is not a grant or a public serialized action reference.
Callback registration/execution remains the subsequent M3 provider.

Records have monotonically increasing local positions, versioned profile-bound
cursors, immutable payload hashes and bounded reads. Retention advances a prefix
floor; old cursors fail explicitly, future/foreign cursors are rejected. Global
capacity refuses acceptance instead of evicting live data. The latest watermark
survives pruning. Original commit-before-ACK evidence is retained even when the
host's retention policy has expired the associated content.

## Ingestion and evidence

An explicit host step checks receiver identity/source and an active lease before
polling, renews between bounded stages, appends a complete batch, then ACKs it.
Append failure never ACKs. Caller cancellation leaves owned database/M2 work to
finish; replay resumes from the same durable watermark. A post-commit uncertain
ACK keeps data, and checkpoint confirmation is conservative. Gap collection
records the gap and returns the opaque M2 recovery capability without guessing
a resync. All errors remain bounded static diagnostics.

Acceptance: strict v7 migration/rollback, source metadata/fixture coverage,
whole-batch atomicity, commit-before-ACK with restart/caller Drop/crash boundaries,
duplicate/conflict/rollback/source-change cases, global Bot+MT limits and pruning,
private callback provenance, primary peer/unknown scope and secret-free projection,
cursor/expiry/hash checks, gaps/baseline/resync, source lease contention/fencing
and graceful native close. Run locked workspace tests, doctests, Clippy/fmt,
Python/schema/SDK verifiers and audit. M3 is not complete until authorized reads,
writes and CLI/MCP consume these records through current policy.

## Implemented evidence

Journal v7 and the real receiver-to-journal ingestion path are implemented.
Nineteen additional tests cover two M2 source/ordinal cases, three strict
migration cases, five storage/projection cases and nine intake cases. The intake
suite uses real encrypted sessions and the owned socket-free RPC fixture; it
includes actual process termination before append, after append before ACK and
after ACK, plus cancellation while the ACK RPC is pending. Restart produces one
copy of the records and conservative checkpoint confirmation. Exact shared byte
capacity includes the eight private callback-query bytes, including query ID zero.

Fresh-context storage/source and projection/privacy reviews have no unresolved
findings. Fixed-size watermark fields are decoded from borrowed SQL blobs before
copying, preserving the full u64 ordinal domain. Source replacement still has an
explicit trusted-host precondition: join the old Client before accepting the new
source. The API cannot prove that join. A storage fingerprint identifies the
store, not backup freshness; coherent session/application restore remains the
host's responsibility. Cross-model review was skipped in this autonomous run.

Windows/Rust 1.98.0: 555 all-feature / 365 default workspace tests, 15 doctests,
clean Clippy/fmt, isolated storage/MTProto builds, deterministic protocol/agent
schemas, 25 Python checks, four pinned SDK verifiers and audit of 363 dependencies
pass. The all-feature executable is rebuilt after schema verification. No live
Telegram calls or performance/platform acceptance are claimed. Authorized event
disclosure, persistent MTProto write admission, callback/file providers, action
execution and CLI/MCP dispatch remain M3.
