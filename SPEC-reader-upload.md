# One-shot AsyncRead upload (client / Q1 T15 prerequisite)

## Objective

Expose the architecture's bounded one-shot streaming source through InputFile.
The documentation corpus exposed that the public variants currently only accept
bytes, owned bytes, paths and file IDs. Keep T15, implement this capability as a
separate client slice, then validate its executable reference before corpus freeze.
Do not count draft feasibility or reference runs as an agent success score.

## API and ownership

Add one constructor:

```rust
pub fn reader(
    name: impl Into<String>,
    reader: impl tokio::io::AsyncRead + Send + 'static,
    length: u64,
) -> InputFile;
```

The reader is owned once; no Clone/reopen/replay factory or full-file RAM copy.
Accept !Unpin and !Sync readers by boxing/pinning internally. Preserve existing
InputFile Send+Sync auto traits: a private std Mutex can store the owned pinned
reader until into_part consumes it, before any await/poll. No lock is held across
network work. Other source behavior and all agent file-handle APIs stay unchanged.
Arbitrary AsyncRead implementations are trusted Rust code; this is not a new
raw-stream tool or permission boundary for an untrusted agent.

## Byte and error contract

- length defines exactly the prefix to upload. Preflight validates name and the
  existing remaining upload budget before polling the reader. Zero length does
  not poll it; after length bytes, do not probe/read the suffix or wait for EOF.
- Use Tokio Take to enforce that prefix plus a small AsyncRead adapter that
  reports premature EOF while bytes remain. ReaderStream provides bounded
  chunks/backpressure; never read_to_end or stage a copy on disk.
- All sources use the existing shared attachment preparation/request path,
  semaphore and absolute deadline. Local bad name/size is NotSent; read errors,
  partial upload, timeout and cancellation after dispatch remain Unknown.
- Sanitize reader I/O failures before they can enter body-error diagnostic
  chains. Public Error contains only existing safe kind/field/outcome data;
  reader error strings/sources, credentials and payload bytes are never retained.
- Attachment retry refusal applies before reading this source, for direct
  send_document and generated named/nested attachment calls. No automatic
  replay, detached library producer task or widening agent grants.
- The private reader part uses reqwest Part::stream with an unknown HTTP part
  length, while length remains our enforced prefix/budget. The authenticated
  reqwest 0.13.5 source computes known Form lengths with unchecked u64 additions;
  do not introduce a caller-controlled u64 overflow through stream_with_length.
  Exercise length=u64::MAX with an explicitly large configured budget and a tiny
  short reader: fail safely, never panic/allocate by declared size. HTTP/1.1 may
  use chunked transfer; do not change existing bytes/owned/path wire behavior.
- The reader drops when the consuming request is dropped/cancelled/completed.
  External producer tasks remain the caller's responsibility. Reader source
  polling never happens merely by constructing/dropping a lazy descriptor.

## Tests, scope and commands

Read src/files.rs, shared request/typed attachment code and existing tests first.
Add behavioral RED tests before implementation: happy exact prefix/no suffix
read, zero, premature EOF/error redaction, !Unpin/!Sync reader, invalid/oversized
preflight with zero polls, pending reader deadline/cancellation/drop/capacity,
generated attachments and retry refusal. Use synthetic data and localhost or
MockTransport, never Telegram. Verify both production HTTP and in-memory paths;
MockTransport's body-capture ceiling is not a large-upload memory benchmark.
Add compile assertions for preserved auto traits and a small runnable stream
example/recipe. Keep schema/SDK source, dependencies and Cargo.lock unchanged.

```
cargo test --locked -p tg --features test-kit --test reader_upload
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --all-features --all-targets
cargo test --locked --workspace --all-features --doc
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
python -m unittest discover -s tools/tests -v
python tools/schema.py check
cargo audit
```

Use bounded separate targets/evidence, check the 20 GiB reserve/budget, and clean
only terminal owned build caches with Cargo. Verify the new surface on Rust
1.89.0 and current stable; broader native matrices remain the compatibility gate.

Sources: pinned local reqwest 0.13.5 async_impl/multipart.rs compute_length/
PartLength; [Tokio Take](https://docs.rs/tokio/1.54.1/tokio/io/trait.AsyncReadExt.html#method.take),
[ReaderStream](https://docs.rs/tokio-util/0.7.20/tokio_util/io/struct.ReaderStream.html),
[reqwest multipart](https://docs.rs/reqwest/0.13.5/reqwest/multipart/index.html).
