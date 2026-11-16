# tg-mtproto

Separate MTProto package, currently implementing M2. Encrypted local session
storage and the owned network Client with typed raw TL calls are implemented.
Bot login and live identity verification are also available. Interactive user
authentication, common actions, files and observable update recovery remain open.
This package does not route MTProto operations through the Bot API.

```rust,no_run
use tg_mtproto::{Session, SessionBinding, SessionKey, SessionOptions};

async fn open_session(
    protected_absolute_path: &std::path::Path,
    host_key: SessionKey,
) -> Result<Session, tg_mtproto::SessionError> {
    Session::open(
        protected_absolute_path,
        SessionBinding::new("personal", 12345)?,
        host_key,
        SessionOptions::default(),
    ).await
}
```

Provision a cryptographically random 32-byte key through a host secret store.
Keep it outside this database, tool arguments and logs. The host protects the
database directory and backups. Opening performs no network request; remembered
account metadata is not proof of current authorization. The forthcoming common
actions must verify the expected identity before accessing account data.

Trusted Rust code can invoke the pinned TL layer directly:

```rust,no_run
use tg_mtproto::{Client, ClientError, ClientOptions, Session, tl};

async fn probe(session: Session) -> Result<tl::enums::Pong, ClientError> {
    let client = Client::new(session, ClientOptions::default())?;
    let response = client.invoke(&tl::functions::Ping { ping_id: 42 }).await;
    client.close().await?;
    response
}
```

Construction is lazy and claims a single client per session. The first polled
call starts the driver; Tokio timers are required. Defaults allow 16 owned jobs,
a 30-second absolute deadline, a 1 MiB encoded request and an 8 MiB response.
Admission precedes serialization. Dropping a caller does not release capacity
while its owned work remains active. A timeout or session fence closes the whole
generation; `close().await` joins the driver tree before draining storage.
Public `Session::close` refuses while attached: close through the Client.

Raw calls do not retry. An ambiguous remote effect returns `Outcome::Unknown`;
a confirmed RPC refusal is `Rejected`, and a locally unsent call is `NotSent`.
Diagnostics retain static kinds/codes and allowlisted numeric flood waits only.
Raw TL is a trusted host capability, never a generic agent tool. Encoded response,
gzip expansion and envelope nesting limits apply before SDK control/update
decoding. Arbitrary Rust deserializers are trusted; decoded TL heap/stack fuzzing
remains a release gate.

Build this development package from the repository workspace. Its hardened
grammers dependency graph currently uses workspace patch overrides; external
distribution is still a release gate. Do not remove those overrides.

`verify_account(expected)` queries the current self user and checks its ID/kind
against the host expectation and remembered account. It commits matching identity
and peer metadata together before returning. `sign_in_bot(credentials, token,
expected)` accepts host-only `AppCredentials` and `BotToken`, checks the app
binding/token prefix and validates the returned self identity. Credential Debug
is redacted; no plaintext getter or serialization is provided. Authorization
transitions exclude raw/identity jobs until their final outcome is classified.
An unknown transition fences the generation before waking waiting jobs. The
phone/code, 2FA and DC migration flows remain in development; see
SPEC-mtproto-auth.md. Agent tools cannot initiate login.

`sign_out(expected)` verifies the live account, durably quarantines the session
before LogOut and ends the generation. Success waits for joined network jobs,
all admitted storage writes and final credential clearing. Unknown/refused or
timed-out logout leaves durable quarantine. `Session::is_quarantined` reports
that local policy state; ordinary Client construction/admission then refuses.
After closing/reopening, a trusted host may explicitly call
`forget_quarantined_authorization` to discard local keys and checkpoints. This
does not revoke unresolved remote authorizations and never replays logout.
See SPEC-mtproto-logout.md. Snapshot payload v2 requires quarantine state; old v1
payloads migrate strictly while the encrypted envelope/SQLite format stays v1.

Snapshots use full-round XChaCha20Poly1305 with a fresh OS nonce and authenticated
profile, application ID, storage ID and revision. A native SQLite worker commits
before publishing the cache. Its bounded jobs retain their slot after caller
cancellation. A heartbeat maintains ownership; expired or failed owners fence
all clones. `close().await` stops admission, drains admitted work and closes the
connection. Dropping the last handle initiates the same drain without blocking
the runtime. Crash recovery waits for the persisted lease to expire.

Backend working update state and application acknowledgements are separate.
Login, peer caching and reads cannot commit a provisional checkpoint. Whole-file
backup rollback and copied credentials cannot be prevented by a per-file owner
lease. Zeroization covers owned key/plaintext buffers on a best-effort basis;
the runtime, operating system and upstream copies are outside that guarantee.

Run `cargo test --locked -p tg-mtproto --all-targets` from the workspace root.
Tests use synthetic keys, actual encrypted SQLite files, a killed child process,
the SDK-backed RPC fixture and an encrypted loopback connection. No Telegram
account or live server is contacted. The full contract is
in SPEC-mtproto-client.md and SPEC-mtproto-session.md at the repository root.
