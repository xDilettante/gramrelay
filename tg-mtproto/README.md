# tg-mtproto

Separate MTProto package, currently implementing M2. The first usable increment
is encrypted local session storage; a network Client is still in progress.
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
account metadata is not proof of current authorization. The future Client must
verify the expected identity before admitting account actions.

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
Tests use synthetic keys, actual encrypted SQLite files and a killed child
process; no Telegram account or live server is contacted. The full contract is
in SPEC-mtproto-client.md and SPEC-mtproto-session.md at the repository root.
