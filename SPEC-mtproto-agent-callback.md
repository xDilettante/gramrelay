# Authorized MTProto callback answers (M3)

Extends SPEC-mtproto-agent-actions.md and SPEC-mtproto-agent-read.md. Add the
seventh Rust executor action, answer_callback_query, using real M2 TL codecs and
the existing v8 canonical-callback admission index. No journal schema change,
live Telegram action, raw tool query ID or extra authority source.

## M2 typed intent

Add redacted CallbackAnswer with an opaque signed i64 query ID (all bit patterns,
including zero), optional plain text up to 200 UTF-16 units and alert flag. No
URL/redirect/game/payment options; cache_time is zero. Text validates before
dispatch and is wiped with owned request buffers. Account::answer_callback and
answer_callback_until use the shared owned run path with live expected identity,
NoRetries and an optional absolute monotonic cap covering identity/queue waits.
Expected user accounts refuse locally before RPC. Only server Bool true confirms
success; false/malformed/late replies are Unknown. RPC refusals retain M2 outcome
classification. Rust intent data is not an agent permission or observation proof.

## Observations and references

Retain private callback observation metadata only for selected authorized records
inside Reader pages: query, reviewed primary peer, original receipt/provenance and
profile cursor. It has no Serialize/Deserialize path. Public pages still contain
only allowlisted event data; the plain Reader does not issue action references.
The executor consumes this metadata without a second storage/RPC read and adds
an optional opaque callback_reference to each eligible selected event. Missing,
expired, conflicting or capacity-limited references omit that metadata without
losing authorized untrusted events. A host may also observe a StoredEvent from
the exact same journal instance/profile. Neither sender nor actor creates scope.

Only an MT bot profile can issue a reference. Observation checks current read OR
answer authority for the actual peer, or explicit unscoped_updates for inline
callbacks. It is not approval. References bind principal/profile, exact bot,
canonical query hash, peer/unscoped boundary, original receipt and expiry. Use
random opaque IDs, canonical per-owner deduplication and a bounded two-way lookup;
default capacity 1024, positive maximum 65536. Same query/scope/time returns the
same reference; contradictions fail without rebinding or evicting live entries.

Reuse CallbackClockPolicy: default JournalMonotonic requires original same-journal
collection provenance. It disappears at journal restart; duplicates/rereads never
refresh it. Explicit host TrustedWallClock permits historical/cross-process
observations with original receipt time and host clock responsibility. TTL defaults
to 10 seconds, minimum 1 ms and maximum 60 seconds. Clamp wall expiry to original
monotonic observation plus TTL when available, then retain an immutable monotonic
deadline and a registry wall high-water shared by every copied reference and
authorization. Observed expiry is irreversible under clock rollback. Sample
runtime grant/approval time after acquiring policy locks and recheck the original
reference after waits. Do not claim Telegram-side creation age or
server acceptance. Raw query IDs, origins and deadline proofs never serialize.

## Authorization, durable admission and recovery

Answer requests accept only opaque reference, optional text and alert. Lookup is
owner/profile checked after current base action/account checks. Execution and
approval use the stored original Scope, not self-attested peer arguments. Each
current grant/approval check also verifies reference wall/monotonic expiry.
Approval cannot outlive the reference or its remaining local deadline. GetOperation
uses retained scope independently of a live reference and does not renew the
original write approval. User account grants cannot include answering callbacks.

Preserve the shared prepare/begin/complete pipeline. Canonicalize the ephemeral
alias in the durable intent to the domain-separated query hash; include original
receipt/expiry and all semantic answer fields. Approval still binds the exact
visible Request and alias. Reissued host references can resume the SAME confirmed
NotSent key with unchanged semantic metadata, while changed targets/options/times
conflict. The v8 partial index seals canonical query admission globally across
principals, aliases, registries and restarts. NotSent may retry its own key; no
new key can bypass an already admitted query, especially after Unknown/Rejected.

Before begin, build the immutable answer intent without a target-peer RPC. M2
verifies bot identity inside the capped owned action after durable admission.
Wrong identity/closed backend is a classified failed attempt, not permission to
guess or send under another account. Cap RPC by the operation lease minus receipt
margin AND the original reference deadline. Policy/queue/identity waits cannot
extend either. Retain owned cancellation capacity and resumable joined close.

Store only acknowledged=true on confirmed success; all failure advice remains
static and bounded. Cached/query-state disclosure still checks current original
account/action/peer and applicable approval. The final caller-side revision guard
also protects callback-bearing read pages and buffered answer receipts. An expired
reference cannot start a new answer or read a cached answer through that action;
get_operation can inspect the retained receipt under current grants.

The v8 fence is per profile within one journal database; separate journals or
restored forks do not share it. Duplicate source-watermark replay and stored-row
rereads retain provenance. A repeated raw query ID in a distinct later server
batch is a new observation, not proof of original server age. A conflicting live
registry entry refuses changed scope/receipt metadata; any previously admitted
query remains durably fenced after RAM expiry/restart. No unbounded all-time
observation ledger or cross-journal exactly-once guarantee is claimed.

## Acceptance

M2 tests cover exact wire flags/query/text/cache/Bool, user refusal, local input
bounds, deadline during identity wait, false/RPC/timeout and caller Drop without
replay. Provider tests cover same-origin/profile, current bot/scoped/unscoped
authority, canonical ID reuse/conflicts, unknown IDs, foreign owners, TTL/clock
rollback, capacity/pruning, private serialization, restart default refusal and
explicit wall-clock restoration. Real M2 intake must issue usable references
through executor reads and host observation. Test approvals/revocation, false/
Rejected/Unknown no replay, cross-principal alias admission, stable semantic
NotSent recovery after reference reissue, reference-capped identity waits, joined
shutdown/cancellation and retained get_operation after expiry. Run locked full/
default suites, docs, fmt/Clippy, isolated builds, protocol/agent schema and SDK
checks, audit and a socket-free authorized callback example. Document remaining
chrono advisory and open document/tools/release work without claiming completion.

## Local verification outcome

Implemented the seventh Rust executor action and M2 CallbackAnswer. Six M2 cases,
seven registry/runtime unit cases, two Bot API clock regressions and ten real-TL
intake/executor integration cases add 25 checks. Full workspace: 613 all-feature
and 375 default tests, 16 doctests. Final focused all-feature library/integration
checks also cover read-only observation without answer permission, answer-only
host observation, buffered disclosure and original source-time sampling.

The shared-clock and post-policy-lock time defects each have failing pre-fix
regressions and passing corrected checks. Fresh contract, execution and registry
re-reviews found no remaining substantive findings. Cross-model skipped:
non-interactive context. Formatting, Clippy, isolated storage/executor/MT builds,
25 Python checks, 193-method/425-type schema and all four pinned SDK verifiers
pass. The socket-free mtproto_authorized_callback example commits real TL intake
and dispatches exactly once for two invocations. Agent schema verification confirms
three unchanged v1 artifacts/eight Bot tools; all-feature CLI/MCP binaries were
rebuilt afterward.

Fresh cargo audit scans 363 dependencies, exits zero with one informational
RUSTSEC-2026-0543 chrono-unmaintained warning. No clean-audit, live-server,
performance or cross-platform claim. M3 document provider/CLI/MCP and Q1–Q4 remain
open; the full completion goal remains active.
