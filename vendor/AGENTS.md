# Vendored source

grammers-mtsender is pinned to the published 0.12.0 archive and has a narrow
downstream lifecycle/fixture patch. A separate grammers-mtproto 0.12.0 patch
enforces bounded incoming parsing. Keep parser validation changes isolated
and source-verified; never modify cryptographic primitives or key derivation.
grammers-tl-types has a separate primitive-decoding patch. Preserve its schema,
generator, build script and serializer bytes; validate changes against the pinned
archive. Its safe --lib tests use root target/vendor-tl.
Check tools/verify_grammers.py after changes; keep the recorded diff reviewable.

The upstream integration tests/lib.rs contacts real Telegram. Do not enable
upstream-live-tests, run it or supply real credentials. Local validation uses:
`cargo test --locked --manifest-path vendor/grammers-mtsender/Cargo.toml --lib`.
Use CARGO_TARGET_DIR pointing at root target/vendor-sender, not this source tree.
The mtproto core's safe test command uses --lib and root target/vendor-protocol.
Its upstream dependency-documentation integration test also assumes a full
upstream repository layout; do not run it from the published crate snapshot.
