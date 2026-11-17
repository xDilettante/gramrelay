# Quarantined MTProto logout (M2)

Implement the pending logout/session lifecycle in SPEC-mtproto-auth.md. The
existing owned authorization gate, deadlines, NoRetries and joined shutdown
remain mandatory. Use synthetic accounts and temporary encrypted stores only.

## Persistent state and compatibility

The subsequent update-journal increment (SPEC-mtproto-updates.md) upgrades this
payload to v3 while preserving required quarantine and the v1 envelope. The v2
description below records the logout migration; current readers accept exact
v1/v2 upgrades and exact v3, with no fallback or unknown-field acceptance.

Snapshot payload v2 adds a required `quarantined: bool`. SQLite schema, encrypted
envelope format and AEAD context stay v1. Decode an authenticated version first,
then use the exact v1 or v2 structure with recursive unknown-field rejection and
end-of-input validation. V1 upgrades in memory to v2/false and persists v2 on its
next mutation. V2 missing/null quarantine or future versions fail closed; never
fall back to v1 after v2 decoding fails. Legacy key buffers still zeroize on Drop.

Session::is_quarantined reports local policy state, not remote authorization.
Normal Client construction and admission refuse quarantine. The shared RPC path
checks it again after waiting for the auth gate. Only the private logout RPC can
bypass this check. Public Session opening/metadata/close remain available for
inspection and explicit host recovery.

## Sign-out ordering

Client::sign_out(expected) owns an exclusive auth job. Check stored expectation
and verify the live self identity inside that job before the logout intent.
Then arrange generation stop on every completion path and durably commit
quarantine BEFORE auth.logOut admission. Calls already holding shared access
finish before the exclusive gate; queued/new ordinary calls must not dispatch
after the intent. Never replay an uncertain RPC or automatically clear quarantine
on a 401/refusal, timeout, cancellation, panic or broken connection.

A valid LogOut response marks confirmed completion. Discard/zeroize its optional
future-login token; this library does not persist login shortcuts. The owned
runtime classifies the final result with the gate still held. Accepted success
arms credential clearing, seals the generation and releases the gate only after
publishing its stop. A late/failed result does not arm successful finalization.

Cleanup order is: quit and join the whole driver tree; abort/join residual client
jobs and join running blocking computations; close native admission and drain every already admitted storage mutation;
then run a native close finalizer that atomically clears all DC keys, peers,
identity and acknowledged/provisional update state and resets quarantine. This
prevents a late backend key/peer mutation from resurrecting cleared credentials.
The finalizer intent is private and sticky, installed before storage ownership
changes from attached to closing, so a concurrent public close cannot bypass it.
Always close/release the store even if finalization fails; report Unknown and
preserve quarantine unless the clearing transaction actually committed.

Successful sign_out waits for Client::close and its finalizer. Cancelling that
wait cannot cancel owned cleanup. Failures can return conservatively while
cleanup still owns capacity; close().await remains the explicit barrier. A
failure of preliminary identity verification sends no LogOut and creates no
logout quarantine. Once the intent is committed, absence of an accepted valid
response leaves quarantine durable across reopen.

Final outcome acceptance and supervisor failure sealing share one per-job
decision lock. A sealed timeout cannot later arm clearing; an accepted result's
receipt is not replaced by shutdown. Revalidate pending completion when a timer
fires. Only static error metadata is handled under this lock; user result values
are delivered/dropped afterward inside their owned job.

## Explicit host recovery

Session::forget_quarantined_authorization is a deliberate local discard, not a
remote logout or revocation. It refuses attached/nonquarantined sessions, holds
unique ownership in its owned native job even after caller cancellation, clears
the same credentials/state atomically, then permits a fresh Client/new keys.
An unresolved remote authorization may still exist; operators can revoke it in
their Telegram account. There is no automatic discard, resume or replay.

## Verification

Prove strict v1 migration/v2 required state and authenticated rejection of bad
formats; intent-before-RPC; successful full wipe after queued late backend writes;
Unknown/refused/timed-out logout across encrypted restart; concurrent admission
and close; cancellation through finalization; explicit discard ownership and
no-network behavior. No success may precede the joined native finalizer. Keep M2
open for interactive auth, DC migration, common actions/files/history and updates.

Increment evidence: 51 tg-mtproto tests pass, including strict payload migration,
quarantine/discard restart, cancelled discard ownership, finalizer-after-queued
mutation, failed-finalizer ownership release, intent-before-LogOut, cancelled
waiters and timeout/lost/refused/late logout. The multithread final-acceptance
regression failed before synchronization and passes after. Independent storage
review found no concrete defect; runtime review found and verified the acceptance
race fix. No live account/server or release-readiness claim follows.
Windows/Rust 1.98.0 workspace gates: 394 all-feature / 230 default tests, five
doctests and clean all-feature Clippy/fmt. The dependency graph is unchanged.
