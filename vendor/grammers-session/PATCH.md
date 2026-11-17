# grammers-session 0.12.0: observable backlog

Published archive SHA-256:
`221a58eaf6bfb61ecd1cb8bc58c20d2bc7ec3749b968bc90d52bcf9a8547d4e6`.

MessageBoxes exposes normalization and retained event count/encoded size without
changing state. MessageBox exposes counter metadata before the upstream zero-PTS
filter, allowing a caller to validate even unordered records. The underlying
from_update filtering stays equivalent. Math for sort keys and seq/pts comparisons
uses i64 intermediates so i32 inputs cannot silently wrap in release or panic in
debug during duplicate handling. These preserve mathematical sequence comparisons;
no custom update sequencing, storage, schema or cryptography is introduced.

Cargo.toml adds only the sibling hardened TL override for standalone library tests.
All other upstream source bytes are unchanged. The application uses bounded
input/output accounting, a privacy dispatcher and explicit durable gap handling.

Verify with `python tools/verify_grammers.py check --package grammers-session`.
Local real-TL receiver tests exercise the observation through the production
MessageBoxes path. No live upstream integration test is authorized.
Twenty original library tests pass with a separate CARGO_TARGET_DIR at
target/vendor-session and:
`cargo test --locked --manifest-path vendor/grammers-session/Cargo.toml --no-default-features --features serde --lib`.
