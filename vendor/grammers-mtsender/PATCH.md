# Local mtsender safety patch

Upstream: grammers-mtsender 0.12.0, published archive
https://static.crates.io/crates/grammers-mtsender/grammers-mtsender-0.12.0.crate
SHA-256 `9a6339a64b7b70ae4447ae4ae7b1b5a1e2e15649882bf26550df356e7a8bb1a4`.
Published VCS commit: `e61cf8585e1e9969ab81eda9fb4725a1500e2d86`.
Upstream copyright headers are retained. Distributed under Apache-2.0.

Existing source changes are limited to sender_pool.rs, sender.rs,
configuration.rs and the normalized Cargo.toml:

- Quit uses an independent sticky signal, interruptible during connection setup.
- Completion closes admission, aborts and joins all child senders. Unwinding is
  caught until that drain completes, then propagated. futures-util supplies
  catch_unwind; no protocol or cryptographic primitive changes.
- Failed nonblocking update forwarding latches an independent loss notification,
  including lost ConnectionClosed events. The signal never resets in a generation.
- Publishing is disabled. Upstream live integration tests require an explicit
  upstream-live-tests feature; it is not used by this project. Local tests use
  synthetic state/loopback and multi-thread Tokio test support.
  The upstream dependency-documentation integration test also requires its full
  repository layout (an includes file is absent from the published archive), so
  it is gated together with those upstream-only tests.
- Response limits reach plain authentication setup and every encrypted sender,
  including the 404 fresh-key path. Setup RPCs and incoming updates require exact
  TL consumption. Malformed update/control input emits a loss marker. The
  separately pinned mtproto/TL patches enforce bounds and strict primitive
  decoding before remote control/error/update objects are constructed.

The recorded tg-safety.patch is checked against the hash-verified archive with
`python tools/verify_grammers.py check` (cached archive), or add `--download` to
fetch that exact archive. Generation is explicit, developer-only:
`python tools/verify_grammers.py generate`. Review the resulting diff.

Safe local test command, from the repository root:

```powershell
$env:CARGO_TARGET_DIR = "$PWD/target/vendor-sender"
cargo test --locked --manifest-path vendor/grammers-mtsender/Cargo.toml --lib
```

An owning adapter must call quit and await run. Externally aborting or dropping
the run future still cannot join children; no asynchronous destructor can do so.
This patch alone does not provide application acknowledgement, gap recovery,
bounded RPC admission or safe reconnect. Those belong to tg-mtproto.

The optional downstream `tg-test-kit` feature adds `SenderPool::with_test_rpc`
and `SenderPoolHandle::test_push_update` for synthetic tests. RPC callbacks run
concurrently in the same owned child tree; fixed NoRetries and ordinary SDK
serialization/typed response decoding remain in use. The callback factory must
construct its future without blocking. A weak update capability preserves queue
EOF when the runner closes. The production Client does not select this mode;
tg-mtproto currently enables the feature only in dev-dependencies. The local
tg_rpc_fixture.rs file is opt-in support, with no upstream crypto changes.
