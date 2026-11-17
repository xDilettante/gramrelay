# Observable MTProto updates (M2)

Status: implementing. Extends SPEC-mtproto-session.md. This specification covers
the complete receiver/recovery path; incremental storage work alone does not
complete M2. No live session or Telegram dispatch is authorized.

## Application boundary

Use maintained grammers-session MessageBoxes with raw SenderPool updates, not
the SDK UpdatesReceiver/UpdateStream. Resume loads only the durable acknowledged
checkpoint. With no checkpoint, Resume returns an explicit baseline requirement;
StartNow is a host decision, never an automatic GetState fallback. Initial state
is presented as an empty baseline batch and requires explicit acknowledgement.

One receiver may be active per Client; owned reads keep that claim after caller
cancellation. Whole batches preserve order. The receiver shares Client admission,
live expected-self verification, authorization read guard, absolute RPC deadlines,
request/response bounds, NoRetries and joined shutdown. Idle polling returns None;
it does not turn a quiet Telegram connection into a failed request by itself.
Receiver capabilities bind account/client/auth epoch and never expose authority
through JSON. M3 will append safe records to the agent journal before ACK.

## Durable batch journal

The authenticated Session snapshot moves to payload version 3 (the SQLite/AEAD
envelope stays version 1). Strict v1/v2 readers upgrade to v3 with an empty update
journal; missing required v3 fields, recursive unknown fields, duplicate keys and
unsupported versions fail closed. Old keys, identity, peers, quarantine and ACK
are preserved. No fallback to an older format for invalid new data.

Keep one bounded pending batch in the encrypted snapshot. It contains a nonzero
monotonic serial plus OS-random nonce, exact account, immutable base checkpoint,
candidate checkpoint,
kind (Baseline/Events/Resync) and <=1 MiB of owned encoded events/required peers.
No plaintext event table or public raw session adapter is introduced. Pending
payload buffers are wiped on Drop; serialization/encryption already uses bounded
zeroizing plaintext. Full snapshot bounds still apply. Native worker admission,
lease fencing, cancellation ownership and publish-after-commit remain shared.

Preparing a batch atomically checks expected identity, quarantine, exact current
ACK/base, monotone candidate counters and channel states, and absence of another
pending batch. Events require a baseline; Baseline requires no previous baseline
and an empty payload. Preparing/publishing never advances ACK. A crash before
prepare may redeliver from the old ACK; a crash after prepare replays the same
pending ID/content/checkpoint before any new RPC. External consumers commit their
own effects/deduplication before acknowledging; exactly-once external side effects
are not claimed.

ACK checks exact account/pending ID and atomically advances only that immutable
checkpoint and clears the batch. The latest ACK ID is retained for idempotent
confirmation after a lost response. A stale/different ID cannot acknowledge a
new batch. Nonzero IDs cannot be reused for another batch or gap. Errors are
static and never retain payloads, paths, keys or server descriptions.

## Gaps and explicit recovery

Durably represent scope (account/channel) and reason, including account/channel
DifferenceTooLong, missing channel hash/baseline, malformed input, queue overflow,
backlog bounds and channel access loss. Persist a gap before exposing it. It
blocks new ordinary batches; an already prepared earlier event batch remains
acknowledgeable at its preceding safe checkpoint without clearing the gap.

A host may explicitly choose a new StartNow/resync checkpoint after completing
application reconstruction or accepting skipped history. Store that decision as
an empty Resync batch bound to the exact gap ID and previous ACK. Its ACK clears
only that gap and commits the decision/checkpoint atomically. A newer loss/gap
invalidates a pending resync decision, but never discards an event batch. No gap
becomes a guessed successful empty batch. Overflow's sticky SDK signal requires
a joined new Client generation before normal reception resumes; it cannot be
silently reset in the same driver.

Inspect raw updateChannelTooLong before MessageBoxes: keep existing channel pts;
use a hint only after an explicit missing-baseline decision. Inspect difference
constructors before applying them. Bound SDK retained possible-gap buffers, input
and output batches, channel/entity counts and encoded size. Suppress SDK tracing
that formats event contents; diagnostics may carry bounded reason/count metadata.
No custom MTProto sequencing, cryptography or secret-chat decryption is added.

## Implemented storage evidence

The internal Session journal is implemented; the network receiver remains open.
Payload v3 stores explicit required nullable fields and validates ID ordinal
relationships as well as account/base/checkpoint constraints. Baseline and
Resync are empty decisions: newer loss invalidates either; earlier Events remain
available and acknowledgeable without clearing the gap. Serial high-water marks
survive credential discard. Caller conflicts reject without fencing healthy
storage; invalid authenticated snapshots fail closed.

Payload parsing owns a bounded wiping buffer even on partial/outer decode errors.
Snapshot serialization counts the concrete immutable value before one exact-size
allocation, preventing plaintext copies from growth without wiping the entire
configured capacity on every small write. The intermediate full-limit allocation
caused nine existing deadline/lease regressions under parallel load; exact-size
allocation restored the unchanged full suite. This is local regression evidence,
not the Q2 comparative performance result.

Sixteen new tests cover journal operations, strict migration/schema, partial
payload cleanup and plaintext allocation bounds. Crash tests terminate a real
child before append, after append and after ACK; separate hooks verify no false
success after post-commit lease expiry. Windows / Rust 1.98.0: 492 all-feature /
328 default workspace tests, 11 doctests, Clippy/fmt, 25 Python checks, deterministic
schemas, pinned SDK verification and audit of 363 dependencies pass. Focused
fresh-context review findings were fixed; cross-model review was skipped in the
non-interactive goal continuation. Receiver/M3/Q1–Q4 are not declared complete.

## Required evidence

Storage: strict v1/v2 migration and invalid v3 refusal; exact account/base/ID;
monotone channels; duplicate/lost-response ACK; append-before-ACK/restart;
capacity, quarantine, concurrent stage/ACK, consumer/caller failure, post-commit
lease expiry and crash boundaries; gap/resync invalidation; no plaintext leaks.

Receiver: whole-batch processing, short/sliced/empty differences, StartNow vs
Resume, channel pts hint, missing hash, TooLong, malformed/overflow/backlog,
checkpoint failure, caller Drop, competing receivers, close/drain, and trace
privacy. Mock real TL bytes and synthetic Session only. Existing regressions,
format/Clippy/audit and pinned SDK verification must continue to pass.

Primary sources checked: pinned grammers-session/mtsender 0.12.0 MessageBoxes,
updates_receiver.rs and Session traits; https://core.telegram.org/api/updates ,
https://core.telegram.org/constructor/updateChannelTooLong ,
https://core.telegram.org/method/updates.getDifference and
https://core.telegram.org/constructor/updates.channelDifferenceTooLong .
