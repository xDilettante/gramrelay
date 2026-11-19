# ADR 0022: one verified Rust floor and an explicit native matrix

Status: accepted for Windows/MSRV; other native cells require actual evidence.

All three alpha libraries declare Rust 1.89. The locked dependency graph already
requires it (rusqlite, uuid and aes); dependency metadata is a lower bound, not a
compiler proof. A direct pre-declaration Windows MSVC run passed all 681 tests.
The subsequent declared-MSRV matrix passed default395, all681, doc16 and 25
production feature configurations. Keep Cargo.lock unchanged and test the
complete feature/binary/example surface on that floor.

Use Cargo metadata's feature model instead of reconstructing implicit optional
dependency features. Check each package with --lib and no defaults, each feature,
all features, and separate host/cli/mcp plus MTProto adapters. Workspace all-feature
tests alone can hide production feature-unification errors. The curated matrix
does not claim an exhaustive power set.

Every CLI run has its own evidence directory with compiler/platform/commit/dirty,
lock hash, exact commands, exit codes and logs. Record pending commands before
dispatch. Preserve metadata diagnostics and already collected context on
preflight failure. A failed/interrupted run never inherits another success.
Observed regressions cover implicit features, concurrent logs, interrupted cases
and metadata diagnostics. Three scoped review cycles are clear; cross-model
skipped in this autonomous continuation.

Do not equate an installed stable alias with current stable. rustup check found
the local 1.98 alias behind 1.100.0; install exact current Rust separately, keeping
the user's default unchanged. Native CI covers Linux x64, Windows x64 MSVC and
macOS ARM64 at the floor and fresh stable, with pinned official actions and
read-only permissions. Workflow syntax/review is not platform-runtime evidence.

Rust 1.100 deprecates fetch_update in favor of try_update, which a direct compiler
probe rejects as unstable on 1.89. Keep std's exact checked atomic implementation
and existing overflow semantics, with a reasoned allow(deprecated) limited to
the private advance_epoch compatibility helper. The workspace-wide -D warnings
gate remains. Do not raise the supported floor or hand-roll an atomic retry loop
solely to follow a rename whose replacement is unavailable at that floor.

Retain only small evidence after terminal Cargo cleanup. Local/CI guards require
20 GiB free and target <=20 GiB; a hosted runner whose actual storage is too small
must fail safely. Audit and post-Clippy/relocated stages are guarded too, and
native caches are cleaned before relocated compilation. Never silently weaken
the disk requirement or delete unrelated preinstalled software to make CI green.

See SPEC-compatibility.md and docs/COMPATIBILITY.md for current cell status.
