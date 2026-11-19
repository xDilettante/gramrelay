# MTProto backend spike

Unpublished, isolated experiment for M1. It reads no existing credentials or
private session. Production Bot API dependencies are unchanged. See
[backend decision](../../docs/decisions/0016-mtproto-backend.md) and
[session contract](../../SPEC-mtproto-session.md).

```powershell
cargo run --manifest-path experiments/mtproto/Cargo.toml --locked --bin grammers_probe
cargo run --manifest-path experiments/mtproto/Cargo.toml --locked --bin grammers_io_probe
cargo run --manifest-path experiments/mtproto/Cargo.toml --locked --bin session_crypto_probe
cargo clippy --manifest-path experiments/mtproto/Cargo.toml --locked --all-targets -- -D warnings
cargo audit --file experiments/mtproto/Cargo.lock
python -m unittest discover -s experiments/mtproto -p 'test_*.py' -v
```

The first probe checks actual typed raw API and synthetic session/key/peer/state
serialization. It explicitly records MemorySession's missing self-user lookup;
the production custom Session must implement it. Its network runner is never
polled. The second uses actual encrypted traffic against closing localhost
sockets, with no public datacenters configured: NoRetries makes one connection,
the default policy makes two, and a dropped caller can leave queued work.
These probes demonstrate hazards; they are not production cancellation fixes.
Their assertions run through cargo run; cargo test alone does not execute them.
The crypto probe exercises maintained XChaCha20Poly1305 with synthetic data,
fresh nonces and wrong-key/context/tamper rejection. It is not the session store.

For the native comparison on Windows, use an existing MinGW UCRT C++ toolchain,
CMake/Ninja, Python with zstd tar support and Git. Preparation downloads the
pinned official TDLib source and SHA-256-verified MSYS2 packages into the ignored
repository target/mtproto-spike; it installs no global software. A clean source
tree is required, including staged modifications and unexpected untracked files.

```powershell
python experiments/mtproto/prepare_tdlib.py
cmake --build target/mtproto-spike/tdlib-build --target tdjson --parallel 2
python experiments/mtproto/tdlib_probe.py target/mtproto-spike/tdlib-build/libtdjson.dll
```

Preparation validates resolved destination paths and rejects existing nested
filesystem links before passing trees to native tools; archives use Python's data
extraction filter. The native probe loads only this local build. It calls
synchronous getTextEntities, checks request/response correlation and the
credential-free authorization wait state, and explicitly closes even if a
verification fails. It does not send setTdlibParameters or login requests.

The source/ABI probes and native build are recorded for Windows only. Neither
backend has been authenticated against Telegram. No measured LLM success score,
application recovery guarantee or performance target is inferred from them.
