# Compatibility acceptance (Q3)

## Objective and current evidence

Prove the supported toolchain, platform and feature surface of all three public
libraries without relying on workspace feature unification. Previous evidence is
Windows x86_64 / Rust 1.98.0 only. At the initial audit no rust-version was declared. The
locked all-feature metadata contains 337 packages; the highest declared minimum
is Rust 1.89 (rusqlite, uuid and aes). This is a candidate, not a successful
compiler test by itself. Do not downgrade or silently replace dependencies for this gate.

## Contract

- Declare the same verified rust-version for tg, tg-agent and tg-mtproto. The
  complete lockfile, examples, binaries, tests and optional features must build
  on that compiler. Keep the lockfile and SDK source bytes unchanged unless an
  observed failure requires a separately reviewed fix.
- Run default and all-feature workspace tests, all-feature doctests, and
  production-only library checks for each package with no default features,
  every individual public feature, and all features. Check host+mtproto,
  cli+mtproto and mcp+mtproto independently. Generate cases from actual manifests
  using Cargo's metadata feature model (including implicit optional-dependency
  features), so a new feature cannot be silently omitted. This is a stated combination
  matrix, not a claim that every possible power set has been tested.
- Repeat on the verified MSRV and current stable; record exact rustc -Vv,
  platform, commands, exit codes and logs, including the attempted command on
  interruption with a null exit code. Each CLI run receives a unique evidence
  directory so parallel runs cannot mix summary/log bytes. A failed or interrupted suite must
  never leave an old successful summary as its result. Use only mock/local
  tests, with no live Telegram, private credentials, publish or push.
- Desired native execution matrix: Linux x86_64, Windows x86_64 MSVC, macOS
  ARM64; MSRV and stable on each. A workflow definition or cross-compilation is
  not evidence of native runtime success. Record missing cells as unverified.
- Keep build targets separate from evidence, use repository compact profiles,
  check free space/target budget before full stages and clean terminal alternate
  builds with cargo clean. Do not clean a running target or shared Cargo cache.

## Implementation and acceptance

1. Install candidate Rust 1.89.0 minimally and test the current locked source.
2. Add manifest policy and a small stdlib-only cross-platform matrix runner;
   test coverage generation, failure propagation and stale-result exclusion.
3. Add the explicit rust-version only after actual compiler acceptance. Prepare
   native CI with read-only permissions and pinned official setup/artifact
   actions. Do not push/enable remote work without authorization.
4. Execute available native cells and record exact evidence. Fresh audit,
   formatting, Python/schema and SDK verification complete the local increment.

Commands (runner is introduced by this increment):

```
python -m unittest discover -s tools/tests -v
python tools/check_compat.py --toolchain 1.89.0
python tools/check_compat.py --toolchain stable
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cargo audit
cargo clean --target-dir target/compat-1.89.0 --profile dev
```

Production Rust/API behavior is outside this increment unless real compiler or
platform failures demonstrate a bug. Track the gate in tasks/todo.md,
operator commands in docs/COMPATIBILITY.md and the workflow in
.github/workflows/compatibility.yml. Tool tests live in tools/tests.

## Sources

- https://doc.rust-lang.org/cargo/reference/rust-version.html (support includes
  all targets/features; declared dependency minima alone are insufficient)
- https://doc.rust-lang.org/cargo/reference/features.html#feature-unification
- https://doc.rust-lang.org/cargo/commands/cargo-check.html
- https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/run-job-variations

## Status

The initial direct pre-declaration command on Rust 1.89.0 / Windows MSVC passed:
`cargo +1.89.0 test --locked --workspace --all-features --all-targets --target-dir target/compat-1.89.0`.
Evidence: target/compat-msrv-baseline.log. The three rust-version declarations
were added afterward. The complete Windows/MSRV runner then passed all 28
stages: default395, all681, doc16 and 25 isolated feature configurations.
Evidence: target/compat-1.89.0-evidence/run-7ianukp5/summary.json. Its terminal
cache was cleaned with native Cargo (8.4 GiB); evidence remains separate.
Installed stable alias was only 1.98.0; official rustup check identified current
stable 1.100.0 (2026-11-12), now installed separately without changing the
user's default toolchain. Its complete 28-stage Windows runner passed the same
counts (target/compat-1.100.0-evidence/run-k59txsmg/summary.json).
Linux MSRV runtime acceptance is recorded below; Linux current-stable/tooling
and both macOS cells remain unverified. Full
Q3/Q1/Q2/Q4 remain open.

Runner review findings have RED-to-GREEN coverage: Cargo's implicit feature
model, concurrent per-run evidence, pending command on interruption, and Cargo
preflight diagnostics/context/exit codes. Five focused tests and the full
40-test Python suite pass. The third scoped re-review found no remaining
concrete failures. Cross-model skipped: autonomous continuation.

The read-only six-cell workflow has pinned official actions (checkout7.0.1,
setup-python7.1.0, upload-artifact7.0.2), verified tag SHAs and clean
actionlint1.7.12. Fresh workflow review found missing audit/post-quality disk
guards; those were added, with native cache cleanup before the relocated
consumer and post-stage checks. Native CI execution remains unproven.

Current stable Clippy exposed AtomicU64::fetch_update's rename/deprecation.
A tiny direct compiler probe shows try_update is unstable on 1.89 and compiles
on 1.100. Preserve the unchanged checked atomic operation with a narrowly scoped,
reasoned deprecation allowance on advance_epoch, rather than raise the MSRV or
replace std's atomic loop. General -D warnings remains enabled. The initial
Clippy RED and probe output are preserved; final Clippy and an actual 1.89 crate
check pass. Existing auth epoch-exhaustion runtime coverage passes again on 1.100.
This changes no epoch/ordering/overflow behavior.

Linux x86_64 / Rust 1.89.0 passed all 28 native Rust stages from the private
ff7bfca20752e5dcdfe194cb067663ab2bc4180f source candidate: default395/all681/doc16
and 25 production configurations. All 649 source files were compared against
the validated ZIP manifest after completion and matched. Evidence was copied
to target/compat-linux-1.89.0-evidence/run-megtrgr6, including source-bundle.json
with commit/archive identity and computed counts. Cargo clean removed the
terminal 8.1 GiB dev cache; the original extracted source remains unchanged.

The separate first Linux Python run exposed four harness errors: three tiny
fake executor tests applied the real 20 GiB build reserve to /tmp's smaller
tmpfs; another globally mocked subprocess.check_output and intercepted Unix
platform.platform() before rustc. Scope the guard mock to fake executor tests,
add a real fail-closed low-space test, and use genuinely malformed Cargo
manifests for the diagnostic fixture. Production guard and all Rust sources
are unchanged. Windows41 and the six focused Linux tests pass; the complete
Linux tool suite passed all 41 tests from candidate c5299cc83aa1. A subsequent
actual provenance check exposed missing original upstream SDK archives in a
fresh Cargo home. Those authenticated comparison inputs are now bundled, with
two further regressions: Windows43 and Linux43 plus actual no-cache verifier
checks pass in the working tree. Fresh relocated checking is still required;
see SPEC-source-distribution.md / ADR0023. Linux current-stable's Rust matrix
is running independently from c5299cc; runtime Rust inputs remain unchanged.
