# Bounded incoming protocol validation

Pinned upstream grammers-mtproto 0.12.0 archive:
https://static.crates.io/crates/grammers-mtproto/grammers-mtproto-0.12.0.crate
SHA-256 `fc95dd02ac37101d8121e5098706a9c9b342e855ca7d7b8b5daa91ec7b119649`.
Published VCS commit `e61cf8585e1e9969ab81eda9fb4725a1500e2d86`.
Original copyright headers retained; distributed under Apache-2.0.

The local patch changes Cargo.toml and three incoming-parser files only:
manual_tl.rs, mtp/encrypted.rs and mtp/mod.rs. Cryptographic primitives,
authentication/key derivation, transport and outgoing encoding are unchanged.

The manifest directly binds the sibling hardened TL package, without a registry
fallback or root override requirement. This private source package is not a
registry-ready fork; see SPEC-source-distribution.md.

Encrypted input has configurable RPC/control body, cumulative gzip expansion
and nesting limits. Defaults are 8 MiB, 16 MiB and 32 levels. Incoming object
limits apply to setup/control responses and intermediate inflated RPC objects
too; a deliberately tiny limit can refuse connection setup. Gzip uses bounded
multi-member decoding, validates compressed input and exhausts the cumulative
budget on failure. Malformed RPC errors cannot become confirmed refusals.
Typed control replies and envelopes require complete TL consumption.
The encrypted input parser checks session identity after decryption and
before resetting frame bookkeeping or decoding the body. A mismatch returns
a typed decode error with no session identifiers in its diagnostics.

The sender integration installs limits before InitConnection and after 404
reconnection. Plain authorization handshake responses are checked by Sender
before typed decoding. Limits constrain encoded/expanded bytes, not allocations
made by arbitrary user-provided Rust deserializers. Whole-client lifecycle,
identity, permissions and application acknowledgements are tg-mtproto concerns.

Run source verification from the repository root:
`python tools/verify_grammers.py check` verifies both pinned packages and recorded
diffs. `generate` explicitly refreshes those diffs for review. `--download` may
fetch exact hash-checked archives; `--archive` requires an explicit `--package`.

Safe tests use root target/vendor-protocol with
`cargo test --locked --manifest-path vendor/grammers-mtproto/Cargo.toml --lib`.
The added tests feed synthetic decrypted frames into the production parser;
they do not claim authenticated live Telegram interoperability.
