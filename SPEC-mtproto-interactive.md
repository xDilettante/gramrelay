# Interactive MTProto authorization (M2)

Implement the remaining phone/code/2FA and login migration capability from
SPEC-mtproto-auth.md. Credentials and challenges are trusted Rust host
capabilities, never CLI/MCP tool arguments. No existing private account or live
Telegram call is used to implement or test this module.

## Public flow

Use opaque PhoneNumber, LoginCode and Password types with bounded zeroizing
storage, redacted Debug and no plaintext getter/serialization. App credentials
must match the Session binding before dispatch. A user-login target is either
an independently expected user ID or an explicit NewAccount opt-in. NewAccount
refuses remembered identity; expected targets must match any remembered user.
First login can learn an ID from verified self authorization; this does not
itself register an agent profile or grant permission to act as that account.

Beginning login returns an AuthStep: CodeRequired, PasswordRequired, Authorized,
or an explicit unsupported signup/payment/email/delivery flow. No server URL,
terms, phone pattern or arbitrary error string is exposed implicitly. Code
challenge accessors provide bounded static delivery kind/length and numeric
retry advice. Successful sentCodeSuccess is handled as authorization, not a panic.

Code and password challenges are RAM-only, nonserializable, generation-bound
opaque capabilities. They expire after five monotonic minutes and are bound to
an authorization epoch. A new valid login attempt/bot authorization/logout
invalidates older flows; counters never wrap. Validate binding, epoch and expiry
again after acquiring the exclusive owned auth gate. Copy only the private
zeroizing state needed by the admitted job so dropping the caller/token cannot
cancel surviving work. A confirmed invalid code may be retried explicitly with
the same unexpired challenge; an expired code invalidates it.
Stage changes to PasswordRequired or Authorized invalidate predecessor code and
SRP capabilities atomically under the auth gate, including queued duplicates.
Consume predecessors immediately on the confirmed transition response, before
fetching replacement parameters or persisting authorization. Failed refresh or
commit must never restore the old capability; test retained/queued submissions
after such failure and require a local NotSent refusal.

Code completion sends auth.signIn. SESSION_PASSWORD_NEEDED obtains fresh
account.getPassword state and returns a PasswordRequired challenge. No password
check is sent until the host supplies Password. Successful user authorization
requires exactly a complete, nondeleted self user of the expected kind/ID (or a
valid self user under explicit NewAccount); atomically persist identity/self
peer through the existing helper. Never advance an application checkpoint.

Only allowlisted confirmed RPC names control these branches. Other descriptions
are discarded. PASSWORD_HASH_INVALID and SRP_ID_INVALID do not trigger an
automatic password-check replay: fetch fresh challenge parameters and return a
new PasswordRequired step with a static prompt reason. Old SRP challenges become
invalid. Uncertain authentication fences the generation through the existing
completion decision, preserving all owned resources until joined shutdown.

## Maintained SRP and resource ownership

Use grammers-crypto 0.12.0 calculate_2fa/check_p_and_g; do not implement SRP, KDF,
hashing or modular arithmetic primitives. Check the supported algorithm, all
required fields, p length/high bit, g range, bounded salts and 256-byte public
values before computation. Check safe-prime/subgroup using the maintained helper
and validate public B/A ranges with maintained integer arithmetic. Generate the
private exponent from OS randomness; do not use server bytes as sole entropy.
Unsupported algorithms are explicit; malformed parameters fail closed.

Run CPU work through a private runtime-owned Tokio blocking JoinSet. The auth
gate permits one active computation per generation; reap completed handles before
new admission. Retain input/capacity after caller cancellation. During shutdown,
join running CPU work after driver/client jobs and before releasing admission or
closing storage. Do not block the Session heartbeat worker with SRP computation.
Catch maintained helper panics into static failure. Recheck the absolute client deadline
and closed/fenced/quarantine state before any subsequent auth.checkPassword.
Own password, temporary parameters/exponent/proofs and request buffers zeroize
on Drop; upstream/compiler/OS copies remain outside that best-effort guarantee.

## Confirmed DC migration

Recognize only PHONE_MIGRATE, NETWORK_MIGRATE or USER_MIGRATE with RPC code 303
and a positive bounded DC ID present in the private Session cache. A login action
may follow at most one such confirmed refusal, under its original job/gate and
absolute deadline. Persist the new home DC, disconnect the old one, then resend
the same bounded zeroizing request. Never redirect unknown/5xx/flood failures,
same-DC loops, unrecognized migration names or arbitrary tool-supplied endpoints.
Apply this shared path to bot/code/sign-in/password login RPCs as appropriate;
challenge binding must reject reuse after an unrelated authorization transition.
This explicit confirmed-refusal path is not an uncertain-write retry policy.

## Verification

Use the SDK TL fixture and real encrypted temporary stores. Prove code/2FA wire
parameters and one successful maintained SRP computation, expected/new-account
rules, stale/cross-generation/expired capabilities, code/password refusals,
unsupported branches, migration bounds, caller cancellation, native CPU ownership
and no post-deadline dispatch. Check diagnostics/storage for secrets and confirm
acknowledged state is unchanged. Keep M2 open for common actions, history/files
and observable updates until those real backends and their tests are complete.
Sanitizing production SDK RPC/retry logging is a prerequisite, not a deferred
release gate; verify it through the real Sender error path under verbose tracing.

Implementation evidence: 67 tg-mtproto tests pass, including one actual maintained
SRP computation through the ordinary owned pipeline; the fake server verifies
wire shape and absence of plaintext, not authenticated Telegram compatibility.
Code/password success/refusals, stale/expired/cross-generation and queued duplicate
capabilities, failed parameter refresh, malformed SRP, counter exhaustion,
unsupported signup/payment, original-deadline dispatch prevention and bounded
cross-method DC migration are covered. Owned Vec/String password allocation
transfer has RED-to-GREEN proof after review found an unguarded input copy.
Independent capability, SRP and runtime ownership reviews found no remaining
concrete issue in their scopes. Workspace: 410 all-feature / 246 default tests,
six doctests, clean all-feature Clippy, 25 Python checks, three pinned source archives and audit
of 362 dependencies on Windows/Rust 1.98.0. M2 and release gates remain open.
