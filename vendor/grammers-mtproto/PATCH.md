# Bounded incoming protocol validation

Pinned upstream grammers-mtproto 0.12.0 archive:
https://static.crates.io/crates/grammers-mtproto/grammers-mtproto-0.12.0.crate
SHA-256 `fc95dd02ac37101d8121e5098706a9c9b342e855ca7d7b8b5daa91ec7b119649`.
Published VCS commit `e61cf8585e1e9969ab81eda9fb4725a1500e2d86`.
Original copyright headers retained; distributed under Apache-2.0.

The local patch covers the manifest, authentication clock calculation,
connection.rs, manual_tl.rs, mtp/encrypted.rs and mtp/mod.rs.

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

Clock corrections and sequence adjustments use checked domain arithmetic.
Message IDs retain a nonzero fraction and increase in wire order. A clock
correction that cannot generate a valid next session ID fails decoding.
Local clock or counter exhaustion leaves the output buffer unsendable.
The sender observes a typed terminal failure and closes that session.
The authorization exchange checks the signed server timestamp and the
representability of the offset before computing Diffie-Hellman results.

An internal salt refresh occupies its own packet. Only a reply matching its
completed write can replace the schedule. The receiver validates intervals, stores at most
64 received records and one still-valid carried interval, and checks the
server clock and expiry before choosing an active salt. Expired entries
are removed. An empty reply leaves the last usable salt available during
the refresh cooldown. See the [message identifier rules][message-id] and
[future salt request specification][future-salts].

Completed transport writes record up to 2048 recent child and container
headers for the current session. Standalone receipt packets keep a separate
64-header history so they preserve slots for outstanding regular requests.
A rejection can adjust state once for that
header with the original sequence number and a write age of at most five
minutes. A matching aged rejection releases a pending internal refresh
without applying a correction. Its direct future-salt reply can still finish
the exact outstanding refresh. A valid salt correction or session recreation
supersedes the previous schedule. The recreation preserves an outstanding
refresh if the notice identifies that request as part of the new generation.
Rejected frames roll back applied notice effects, including rejection marks.
Fresh recreation of an established session reports a catch-up signal.

The receipt queue holds at most 16384 distinct IDs, including the batch in
flight. Each `msgs_ack` carries at most 8192 IDs. A completed write consumes its
prefix, while a discarded output buffer leaves the receipts due. A received
frame that would exceed the queue limit rolls back its additions. The sender
can flush a standalone receipt on its next write opportunity even without an
application request. It advances scheduled salts first and alternates priority
when a full-size application request needs the packet. See the [receipt
specification][receipts].

A detailed-info advisory repeats a receipt only for a recently acknowledged
body. Recovery of a body mentioned only in an advisory still relies on peer
redelivery and the higher-level recovery policy.

[message-id]: https://core.telegram.org/mtproto/description#message-identifier-msg-id
[future-salts]: https://core.telegram.org/mtproto/service_messages#request-for-several-future-salts
[receipts]: https://core.telegram.org/mtproto/service_messages_about_messages#acknowledgment-of-receipt

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
