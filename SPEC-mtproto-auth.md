# MTProto authorization (M2)

Implement against SPEC-mtproto-client.md and SPEC-mtproto-session.md. Auth is a
trusted Rust host capability, never agent input. No real Telegram credentials or
live login are used during implementation.

## Identity and bot-login increment

- AppCredentials validates a positive app ID and a 32-digit hexadecimal hash.
  BotToken validates a bounded token with a positive account-ID prefix. Both
  keep owned secrets in zeroizing storage, have redacted Debug, and expose no
  serialization or plaintext getter. Constructors and local validation precede
  networking. The app ID must match the Session binding.
- Client::verify_account(expected) obtains exactly one non-minimal, non-deleted
  self user through users.getUsers(InputUserSelf). Its ID/kind must match both
  the independently supplied expectation and remembered identity. Empty,
  multiple, foreign, partial and inconsistent results fail closed. Matching
  metadata and the self peer commit atomically before success; no update
  checkpoint is advanced. Stored identity alone never proves authorization.
- Client::sign_in_bot(credentials, token, expected) requires a Bot expectation
  matching the token prefix and existing identity before dispatch. It invokes
  auth.importBotAuthorization and validates the returned self user before the
  same atomic commit. Unexpected sign-up/partial authorization is an error,
  never a panic. It does not automatically replay uncertain login.
- Authorization transitions hold an exclusive gate inside an owned client job;
  raw/identity calls hold shared access. Cancellation does not release the gate
  or budget while backend work survives. Deadlines cover gate wait, every nested
  RPC and local persistence. Any future common action verifies expected identity
  inside its job before accessing account data or causing a write.
  The job retains its guard through final deadline/health/outcome classification;
  every Unknown authorization transition publishes the generation fence before
  releasing its writer and waking waiting jobs.
- Raw TL remains a trusted escape hatch; hosts performing raw authorization
  mutations own those additional semantics. It is never exposed by agent tools.
- Local mismatches return NotSent. Remote identity inconsistency after dispatch
  is Unknown and fences the generation. Public errors retain static metadata
  only. Own serialized secret buffers are zeroizing until ownership transfers
  into the SDK; copies made inside maintained dependencies remain best effort.

## Remaining authorization capabilities

Phone/code and 2FA challenges must be opaque, RAM-only, generation/epoch-bound,
bounded-lived capabilities. Use maintained grammers-crypto SRP routines with
validated parameters and owned bounded CPU work; do not implement cryptography.
Confirmed migration refusals may redirect a bounded number of login calls using
only allowlisted migration metadata. Unknown calls never migrate or replay.
Handle unsupported payment/email/signup flows explicitly without panicking.

Before auth.logOut, durably quarantine the session. Confirmed logout plus a local
commit clears every DC key, peer, identity and update checkpoint. Any uncertainty
keeps quarantine across restart. Explicit host recovery must not quietly resume
or replay the old intent. Payload-version migration must be strict and preserve
the existing encrypted file format/AAD; v2 quarantine state cannot default away.

## Verification

Use the SDK TL fixture and actual encrypted Session files. Prove wire parameters,
local no-dispatch refusals, identity mismatch, atomic persisted identity across
restart, caller cancellation, authorization-gate ordering, deadline/fencing and
secret-free diagnostics. Exercise phone/code/password/migration and uncertain
logout/restart before declaring authorization or M2 complete. Run the workspace,
source verification, schema, format, Clippy and audit gates before delivery.

Identity/bot increment evidence: nine SDK-backed synthetic tests pass, including
actual encrypted Session restart and a multithread wake-before-fence regression
that failed before the guard-retention fix. The focused independent re-review
found no remaining defect in that ownership boundary. Interactive auth and
DC migration remain pending; no live compatibility is claimed. Quarantined
logout is implemented separately in SPEC-mtproto-logout.md.
Windows/Rust 1.98.0 workspace verification: 382 all-feature / 218 default tests,
five doctests and clean all-feature Clippy/fmt. Dependency graph is unchanged
from the preceding 362-dependency audit and three-archive source verification.
