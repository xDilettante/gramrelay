# Authorized MTProto document snapshots (M3)

Extends SPEC-agent-files.md and SPEC-mtproto-agent-actions.md. Complete the eighth
Rust executor action using the existing v8 file scope and shared immutable file
metadata/registry. CLI/MCP integration and release gates remain separate work.
No live Telegram action, session/login/path/raw TL authority from tools.

## Shared host snapshots

Reuse RegisteredFile, FileLimits, metadata ownership and capacity accounting from
Bot API. Extract the protocol-independent registry rather than copy its checks.
Register/restore/discard/prune remain trusted-host methods. Profile identity comes
only from the registered Feed. Handles are random, immutable and never reused;
restoration requires exact owner/profile/hash/size. A handle grants no permission.
Journal v9 rebuilds executor_files with the same immutable columns/index and
profile FK to account_profiles, replacing the Bot-only update_feeds FK. Verify
exact v1–v8 schemas and original foreign keys before copying, preserve every
handle/owner/hash/size/name/time plus operations/scopes/budgets, and validate exact
new DDL/FKs before commit. Reject malformed/orphan old data without inferring MT
ownership. Copy/drop/rename/index recreation is one immediate transaction; rollback
keeps the original schema/version/rows. Existing Bot wire/ID meanings are unchanged.

Reserve actual Vec capacity and a snapshot slot before metadata/hash waits.
Cancelled native hash work keeps input and permits until completion. Active upload
owners retain both snapshot and upload permit after discard/prune/caller Drop;
there is no second complete file allocation. Empty snapshots remain compatible
with Bot API, but MTProto registration/restore reject zero-sized documents.
MT names additionally obey M2 protected filename rules. MIME is the fixed
application/octet-stream; tool callers cannot supply MIME, paths or storage data.

## M2 owner and deadline boundary

Add SendDocument::owned_bytes(name, mime, owner) using bytes::Bytes::from_owner.
The trusted owner supplies stable immutable AsRef<[u8]> + Send + 'static data;
Bytes retains it through shared ownership. The host accounts any storage beyond
the exposed slice; Vec entry points retain existing actual-capacity validation
and wiping. Preserve independent expected SHA-256, two-pass hashing, bounded
parts, ordinary failure drain, strict media receipt and NoRetries.

Add Account::send_document_until(peer, intent, absolute_std_instant). Both public
document methods use one private implementation and shared run_until. The cap
covers identity/queue/CPU preparation/parts/final media, cannot be extended by
waits, and does not detach native work. Already sent parts are remote effects;
late/uncertain results remain Unknown. Expired cap is NotSent before dispatch.

Account::cached_peer_until supplies the same cap for pre-begin verified cache
preparation. Accepted Call owns a Bytes guard before identity or action polling;
CallState retains it through action/parts and driver/native shutdown. Failed
queue admission and unstarted calls use native disposal. Native cleanup contains
owner panics and forgets their payload rather than execute an arbitrary nested
destructor; the bounded client stops on cleanup failure. Local constructors and
pre-reservation refusals retain ordinary trusted Rust caller ownership. Failed
enqueue cleanup is caller-owned bounded native work, not an accepted protocol
action; cancellation retains its permit until disposal completes.

Stale queued entries need owned cleanup without fencing an otherwise healthy
driver. Their deadline is not armed for generation shutdown, and they cannot
poll their action. A failure fencing another action must not become their own
RPC response category.

## Durable execution and disclosure

Authorization resolves owner-checked metadata after current base action/account
checks. Grant must include actual peer, action and handle. Exact approval binds
the visible request/handle; immutable file metadata joins the durable fingerprint
and FileScope(handle,sha256,bytes). Dry-run/approve do not require resident bytes,
spend quota or dispatch. They do not promise that a future upload is possible.

prepare precedes fresh intent creation. Existing keys return currently authorized
minimal receipts even when bytes are absent or expiry passed. New attempts require
live metadata, a matching resident owner and active upload capacity before peer
identity/RPC or durable begin. Recheck current rights and file expiry during begin
and immediately before M2 dispatch. Charge full immutable file size atomically
with write/cost admission; cached/get_operation consumes no new upload budget.

M2 cap is min(operation lease minus receipt margin, original file expiry anchored
before policy waits). Same-key confirmed NotSent may resume after exact explicit
restoration; Unknown/Rejected never replay. Owned executor cancellation/close and
final caller-side revision guards remain shared. Expiry after dispatch does not
hide a confirmed retained receipt: expiry gates fresh transmission only.

Validate returned peer, positive message ID, exact document size and SHA-256;
persist/disclose only peer/message metadata. No document ID/access hash/file
reference, filename/hash, caption/content or remote description in receipts.
get_operation retains original current action/peer/file grant requirements without
requiring a live snapshot or repeating the original write approval.

## Acceptance

M2 RED→GREEN proof for owned-source zero-copy/lifetime and absolute caps during
identity/part waits, with no late final SendMedia. Existing Vec/path/checksum/
integrity/drain/caller-Drop cases must remain green. Registry extraction preserves
Bot tests including pending restore and retained upload permits.

Real-TL executor cases cover actual part bytes, exact media fields and receipt;
host validation/ownership/size/capacity/hash restore; current peer/file grants and
approvals; expiry before dispatch and cached receipt after expiry; upload quotas
and restart; missing/restored bytes and same-key NotSent; rejected/unknown no
replay; malformed receipt, reference cap, caller cancellation/discard and joined
close. Run locked workspace all-feature/default/doc checks, fmt/Clippy, isolated
builds, schemas/Python/SDK verifiers and fresh audit; add a socket-free example.
Review source ownership, scope/admission and execution/disclosure independently.
Do not claim M3 or full-goal completion until CLI/MCP and Q1–Q4 are proved.

## Foundation outcome (not completed document provider)

The shared registry and M2 owner/deadline capabilities are implemented. Ten
additional source/lifecycle checks bring M2 to 183 library checks. Pre-fix failures
prove early owner release, skipped session finalization, queued-owner panic and
failed-enqueue caller panic; corrected regressions pass. Tests additionally cover
nested panic payload containment, stale queue without driver fencing, source
pointer/lifetime, failed-read retention and absolute cache/identity/part caps.
The DC-import waiter regression preserves its Closed fence under the new schedule.

Full workspace: 623 all-feature / 385 default tests and 16 doctests. Shared Bot
registry checks (25 library / 12 file integration) preserve prior behavior. Fmt,
Clippy, isolated builds, 25 Python checks, protocol/agent schemas and four SDK
verifiers pass; all-feature binaries were rebuilt after executable checks. Audit
scans 363 dependencies with one known chrono-unmaintained informational warning.
Registry/source/disposal/queue reviews are reconciled; cross-model skipped in
this non-interactive continuation. ADR 0018 records cleanup ownership/trade-offs.

At the foundation checkpoint, remaining work was eighth-action host registration/authorization, exact
file-scope/fingerprint, quota/deadline dispatch and safe receipt verification in
mtproto::Executor; complete the real-TL provider acceptance cases and offline
example above. CLI/MCP and Q1–Q4 remain open. No live-server or performance claim.

## Document-provider outcome

Implemented all eight Rust executor actions. MT host registration/restoration
reuses the shared bounded store, validates M2 names/nonempty data, and supplies
fixed MIME and expected SHA-256 without a second full copy. Current metadata,
peer/file/action/approval, canonical intent, atomic full-size quotas, lease/file
caps and minimal confirmed receipts use the shared write path. Cache and retained
scope remain readable after expiry/discard/restart under current original grants.
Executor joins wrappers/confirmation; host then joins Client/native cleanup and
Journal, in that order. No implicit close of independent host resources is added.

Four new migration cases include observed pre-fix failures for v9 absence,
orphan old metadata and contradictory account kind. The v9 rebuild preserves rows and
all prior migrations (20 migration cases pass). Twelve real-TL integration cases
cover user/bot accounts, ownership/profile/grants/approval/revocation, immutable
restore, memory retention after cancellation, resumed close, cache, expiry,
NotSent/Rejected/Unknown, malformed media, two executors, persistent quotas and
actual journal restart. A policy-lock unit case proves original file cap anchoring
and atomic denial/zero quota/admission after expiry. The socket-free
mtproto_authorized_document example performs one part/media upload and a cached
second invocation after resident discard.

Workspace: 640 all-feature / 389 default tests, 16 doctests; fmt/Clippy, isolated
storage/executor/MT builds, 25 Python checks, protocol/agent schemas and four SDK
verifiers pass. All-feature CLI/MCP binaries rebuilt after executable checks.
Migration, authority and execution reviews found no remaining substantive defects;
cross-model skipped in this non-interactive continuation. Fresh audit scans 363
unchanged dependencies with the known chrono RUSTSEC-2026-0543 informational
warning. No clean audit, live-server, platform or performance claim.

M3 now remains open for shared CLI/MCP dispatch and host bootstrap. Q1–Q4 and the
full completion goal remain active.
