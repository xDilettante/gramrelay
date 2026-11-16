# Vendored source

grammers-mtsender is pinned to the published 0.12.0 archive and has a narrow
downstream safety patch. Never edit crypto/protocol files as part of this patch.
Check tools/verify_grammers.py after changes; keep the recorded diff reviewable.

The upstream integration tests/lib.rs contacts real Telegram. Do not enable
upstream-live-tests, run it or supply real credentials. Local validation uses:
`cargo test --locked --manifest-path vendor/grammers-mtsender/Cargo.toml --lib`.
Use CARGO_TARGET_DIR pointing at root target/vendor-sender, not this source tree.
