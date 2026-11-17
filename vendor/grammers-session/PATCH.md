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

Cargo.toml directly binds the sibling hardened TL dependency for downstream and
standalone use, with no registry fallback or root patch requirement. This private
source package has publish=false; see SPEC-source-distribution.md.
The sole serde_with use (Option<[u8;256]> hex) is replaced with serde with/default
attributes and the already-locked hex 0.4.3. This removes its disabled chrono
integration from the workspace lockfile (Cargo issue rust-lang/cargo#10801).
The types.rs adapter preserves lowercase output, mixed-case input and
None/null/missing behavior, decoding only exactly 512 hex characters into a fixed
array. Malformed string errors are static and never echo key material. Public
DcOption golden tests passed against upstream before the replacement and against
the patch afterward; complete encrypted snapshot/restart tests also apply.
All other upstream source bytes are unchanged. The application uses bounded
input/output accounting, a privacy dispatcher and explicit durable gap handling.

Verify with `python tools/verify_grammers.py check --package grammers-session`.
Local real-TL receiver tests exercise the observation through the production
MessageBoxes path. No live upstream integration test is authorized.
Twenty original library tests pass with a separate CARGO_TARGET_DIR at
target/vendor-session and:
`cargo test --locked --manifest-path vendor/grammers-session/Cargo.toml --no-default-features --features serde --lib`.
