# MTProto agent integration (M3)

Status: implementation in progress. Extends SPEC-executor.md,
SPEC-mtproto-session.md and SPEC-mtproto-updates.md. M2 is complete locally;
this module and Q1–Q4 remain required by the full goal. No live Telegram calls,
private credential discovery, publishing or remote push are authorized.

## Capability map and order

| Module | Responsibility | Depends on |
|---|---|---|
| m3-profile | Exact immutable account identity in the shared journal; distinct current grants and peer scopes | M2, A1 |
| m3-updates | Bounded safe MTProto records, durable deduplication before whole-batch ACK, observable gaps | m3-profile, J2 |
| m3-actions | Typed common actions through M2 with current policy, approvals, quotas, immutable files and retained Unknown | m3-profile, m3-updates, A1 |
| m3-tools | Fixed typed CLI/MCP surface, host-only bootstrap, capability discovery and offline examples | m3-actions, A2, A3 |

Build in this order, using the existing journal/native worker and owned Client.
Bot API v1 arguments and receipts keep their meaning. MTProto account/peer kinds
must be explicit at provider boundaries; a bare number cannot imply either API,
user/bot account or peer kind. No universal raw transport/tool escape is added.

## Profile binding increment

Add host-only `ProfileIdentity` metadata containing one of BotApi, MtprotoBot or
MtprotoUser plus an account ID. It contains no credential, session, access hash,
principal or grant and has no Serialize/Deserialize authority path. Diagnostics
redact IDs. BotApi retains the existing nonnegative ID domain (zero supports
synthetic Bot API fixtures); both MTProto kinds require a positive signed ID.

Journal schema v6 adds `account_profiles`, uniquely binding a profile name to an
exact (kind, account ID). Another profile cannot alias the same exact account;
the same number under another kind is distinct. There is no rebind/delete API.
Binding is idempotent, transactional, bounded by `max_profiles` (default 64,
maximum 1024), and committed before returning. Metadata registration is not live
identity verification, a grant or proof that the account exists. M2 verifies
live identity independently at actual execution.

Strict v1–v5 migration first verifies every prior schema object, then creates the
new table and backfills only known update_feeds as BotApi identities. Never infer
an account for generic legacy operation/budget rows lacking a feed. Reject a
fresh binding over such rows; retained keys/usage keep their old meaning. Existing
feeds, checkpoints, consumers, scoped receipts, file handles, budgets, ownership
and revisions are preserved. Migration can preserve more rows than a newly
chosen capacity; capacity limits new insertions, not idempotent reopen.

`Journal::bind_profile` and `profile_identity` are trusted storage metadata APIs.
UpdateFeed::open binds BotApi in the same transaction as feed creation and option
checks. A failed feed open leaves no new profile binding; a MTProto-bound name
cannot be opened as a Bot API feed. Existing Bot runtime/executor/CLI therefore
cannot reinterpret a MTProto account as a bot with a matching number. Later
MTProto grants, scope rows and file/update storage must check the exact binding
at admission and disclosure, never only the old bot_id column.

## Remaining M3 contracts

The optional `mtproto` agent feature adds separate `mtproto_*` request variants
with explicitly tagged peer kind and canonical decimal-string bare IDs, validated
through M2 constructors. It never reinterprets Bot API chat IDs or options.
Its initial fixed actions are get_me, get_peer, read_updates, send_message,
edit_message_text, answer_callback_query, get_operation and send_document.
An independent immutable request contains all arguments and the operation key;
approval fingerprints use an MTProto domain. Input remains bounded, strictly
object-shaped and redacted. IDs/hashes/tokens/paths cannot become tool authority.

One shared PolicyStore entry per principal/profile contains either a Bot grant
or a MTProto grant. Replacement/revocation changes the same revision domain,
invalidating earlier approvals even across protocols. A MTProto grant binds the
exact ProfileIdentity, actions, typed peer scope, files, expiry, approval needs
and existing durable Budget. A user grant cannot include answering bot callbacks.
Default scopes deny; explicit all-peers/unscoped authority remains host-only.
Local policy checks do not claim a durable admission or live identity check.
Bot operation-scope loads/admission must also match the exact durable BotApi
binding; the legacy bot_id column never establishes an arbitrary MTProto scope.

Only the host supplies an already configured Client, encrypted Session/key,
independently expected account kind/ID and principals/grants/approvals. Agent input
cannot log in/out, import a session, resolve arbitrary host paths, provide keys or
change profile identity. Rust common actions, CLI and MCP must share the same
authorized execution and disclosure path. No compatibility shim may silently
translate unsupported Bot API fields into a different MTProto action.

Update ingestion commits bounded safe records and stable batch deduplication in
the shared journal before acknowledging the M2 receiver. An aborted caller or
crash between these commits replays without duplicate journal records. Never
export raw TL objects, access hashes, file references, credentials or unknown
fields as tool authority. Untrusted event text remains explicitly data. Unknown
or ambiguous event scopes require explicit unscoped authority; content from a
forbidden peer cannot gain authority through forwarded/nested peers. Preserve
observable gaps and their explicit host-only reconstruction/skip decisions.

Action admission rechecks exact current account/action/peer/file scopes and
approvals after preparation and under durable quota/operation transactions.
Scope identity is immutable in retained operations; get_operation rechecks the
original scope. Unknown is retained and never blindly replayed under any key.
The M2 owned deadline, NoRetries, live self verification and joined shutdown
remain authoritative. Callback actions require real host-observed provenance
and the correct bot capability; a user account cannot answer a bot query.

## Evidence and commands

Use synthetic metadata, encrypted sessions, real TL fixtures and localhost only.
Profile tests cover strict migration/rollback, exact kinds, aliases, reopen,
capacity, concurrent binding, legacy orphan keys, Bot feed atomicity and private
diagnostics. Each remaining capability needs its own production-path regression,
including journal-before-ACK restart, scope revocation and uncertain writes.

- cargo test --locked -p tg-agent --test profiles
- cargo test --locked -p tg-agent --test migrations
- cargo test --locked --workspace --all-features --all-targets
- cargo test --locked --workspace --all-features --doc
- cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
- cargo fmt --all -- --check
- python tools/agent_schema.py check (after executable tests)
- cargo audit

Profile metadata alone does not complete M3 or establish live-server, packaging,
performance, memory-bound or platform acceptance.

## Profile increment evidence

Implemented exact host identities, transactional v6 migration and immutable
bounded bindings. Six profile cases cover kind/ID privacy, restart, aliases,
Bot feed rollback/refusal, orphan operation/usage protection, capacity and two
independent SQLite workers. Three additional migration cases prove backfill
preservation, malformed-v5 rollback and v6 constraints/exact-schema refusal;
the previous seven migration cases still pass with the new target version.
The migration regression failed with version 5 before implementation and passes
with version 6. Fresh-context adversarial review has no unresolved findings;
cross-model review was skipped in the autonomous goal continuation.

Windows/Rust 1.98.0: 525 all-feature / 361 default workspace tests, 13 doctests,
clean Clippy/fmt, 25 Python checks, deterministic protocol/agent schemas and
four pinned SDK verifiers pass. Audit of the unchanged 363-dependency graph
passes with `cargo audit --no-fetch` using the advisory database updated in this
run; the first registry/yanked lookup timed out. The all-feature CLI binary is
rebuilt after schema verification. No MTProto executor, policy extension or
tool dispatch is claimed by this profile-storage increment.

## Policy/resource provider evidence

The optional mtproto feature now provides eight immutable prefixed requests,
tagged M2-validated peer IDs and MTProto grants in the shared PolicyStore. One
current entry selects exactly one protocol family; replacement/revocation and
the shared approval matcher retain the same revision domain. Default scopes deny
and bot callback authority cannot be configured for a user account. The legacy
Bot operation scope now carries exact BotApi identity and checks the durable
profile binding on preparation, load/admission and disclosure, preserving v6 DDL.

Eleven further Rust cases include seven public contract/policy tests, two
trusted-provider scope cases, a stored Bot-scope mismatch regression and extreme
M2 peer IDs. Two boundary defects have RED-to-GREEN proof: SDK eager arithmetic
panicked on invalid IDs, and public direct Serde envelope errors echoed unknown
input names. Guards preserve maintained ID ranges; Request/PeerId decoder errors
are static. The ID regression also passes optimized. Fresh scope/ID and policy
reviews have no unresolved findings; cross-model review was skipped in this
autonomous continuation.

Windows/Rust 1.98.0: 536 all-feature / 362 default workspace tests, 14 doctests,
clean all-feature and Bot-only Clippy, storage-only/MTProto-only checks, 25 Python
checks, deterministic protocol/agent schemas, four SDK verifiers and audit of
363 dependencies pass. Bot-only dependency graphs exclude MTProto/grammers.
The Bot v1 CLI catalog still has eight tools and its all-feature binary is
rebuilt after schema checking. These local policy/provider checks do not yet
implement MTProto journal ingestion, durable MTProto write admission, callback
provenance/file registration, an executor or CLI/MCP dispatch; those remain M3.

## Update intake evidence

SPEC-mtproto-agent-updates.md is implemented: safe allowlisted records, strict v7
migration, shared Bot/MT storage limits, fixed-size source watermark, fenced
source ownership and real M2 commit-before-ACK ingestion. Nineteen further tests
include encrypted restart, caller cancellation and actual process kills at all
three append/ACK boundaries. Private callback query/provenance never becomes
serialized authority; replay and restart do not refresh observation time.

Windows/Rust 1.98.0: 555 all-feature / 365 default workspace tests, 15 doctests,
Clippy/fmt, isolated builds, deterministic schemas, 25 Python checks, four SDK
verifiers and audit of 363 dependencies pass. Fresh storage/source and projection
reviews have no unresolved findings; external cross-model review was skipped in
the autonomous continuation. Authorized reads, persistent action scopes/admission,
callback/file providers, executor and CLI/MCP integration remain M3. See the intake
spec for source-replacement and backup-restore host preconditions.

## Authorized event-read evidence

SPEC-mtproto-agent-read.md is implemented as a reusable Reader over the same
PolicyStore, exact Feed and durable budget ledger. Primary-peer/unscoped filtering,
metadata-only cursor checks, approval/revision guards and private projected output
have ten new unit cases plus an extended actual M2 intake integration. A cancelled
call retains its native-job concurrency slot; the regression failed before the
review-driven fix and passes afterward. The final caller-side guard also refuses
revocation while a completed page waits in the reply channel. Fresh re-review has
no unresolved findings; cross-model review was skipped in this autonomous run.

Windows/Rust 1.98.0: 565 all-feature / 365 default tests, 16 doctests, Clippy/fmt,
isolated builds, deterministic schemas, 25 Python checks, four SDK verifiers and
audit of 363 dependencies pass. Persistent write scopes/admission, callback/file
providers, remote action execution and CLI/MCP integration remain M3.

## Durable text-action evidence

SPEC-mtproto-agent-actions.md implements journal v8, exact MT operation scopes and
the six-action Rust Executor: verified get_me/cache-only get_peer, shared Reader,
send/edit text and scoped get_operation. Common key/fencing/budget logic is reused;
MT scopes remain separate from legacy Bot scope rows. Tracked jobs retain capacity
after caller Drop, close can resume joining, and caller-side revision/scope checks
protect buffered replies. M2 absolute send/edit caps cover identity/queue waits.

Twenty-three further cases include actual killed-process recovery at three write
boundaries, caller cancellation/close, scope/approval/revocation, durable quotas,
malformed/Rejected/Unknown replies, cached reads without a backend, v8 migration
and lease/policy wait handling. The offline authorized MT text example sends once
for two invocations. Fresh store/runtime/deadline reviews have no unresolved
findings; external cross-model review was skipped in the autonomous continuation.

Windows/Rust 1.98.0: 588 all-feature / 369 default tests, 16 doctests, Clippy/fmt,
isolated builds and Python/schema/SDK gates pass. The fresh advisory database now
reports informational RUSTSEC-2026-0543 for transitive chrono via rmcp 3.10.0;
the unchanged 363-dependency graph is not claimed audit-clean. Q3 tracks removal
without ignoring the warning. Callback/document capabilities and CLI/MCP still
remain M3, with no live-server or release acceptance claim.
