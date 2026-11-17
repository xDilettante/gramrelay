# External consumers and local source distribution (Q3)

## Objective

A Rust application outside this workspace must receive the exact hardened SDK
graph without copying internal patch rules. Package a relocatable local source
candidate and prove the same property after extraction. This is an increment
toward full release readiness; Q1/Q2/Q4 and Q3 platform/MSRV/registry preparation
remain mandatory separate gates. No publishing, push or live Telegram actions.

Observed before implementation: an independent temporary Cargo workspace using
tg/tg-agent/tg-mtproto selected registry rmcp and all four original grammers
packages, and restored chrono. Evidence: target/external-before-metadata.json.
Cargo ignores patch tables in dependencies; path+version is also unsafe as a
future publishing shortcut because cargo package strips paths and resolves the
registry package. Do not silently ship a package that drops the safety patches.

## Dependency contract

- Make each hardened edge an explicit relative path, including SDK-to-SDK and
  normal/dev edges. Remove root and standalone patch tables. Keep crate names,
  Rust library names, upstream base versions and feature surfaces unchanged.
- Path-only hardened dependencies have no registry fallback. All patched SDKs
  and our private alpha packages have publish=false. Current distribution is
  the complete source checkout/archive, usable through a path or pinned Git
  dependency. A registry-ready release still needs separately identified patched
  SDK packages (or reviewed upstream releases), final package metadata/license
  and normalized .crate verification; this increment must not claim that gate.
- No source behavior, public method/error, crypto, retry, schema or lifecycle
  changes. Existing archive/diff verifiers cover every intentional manifest
  change; preserve all other upstream bytes and license/provenance artifacts.
- External metadata must contain exactly one instance of each of the five
  hardened packages, at the expected source-tree manifest with no registry
  source for path consumers. Also verify a local file-URL Git consumer pinned
  to the exact committed source: the eight library/SDK nodes must share that Git
  source/commit and expected relative manifests in its checkout. Every resolved
  edge to hardened SDKs must use that instance. No chrono in the
  external consumer lockfile/closure; no consumer patch/config source override.
- Optional test-kit must also compile outside the workspace. A runnable external
  Rust fixture uses public Bot/agent/MT APIs with synthetic data and owned local
  lifecycle only. It must exercise a mocked Bot action and MT real-TL fixture,
  then close owned resources; no network/login or credentials are needed.

## Source candidate contract

- Export a clean, committed Git snapshot with git archive; no build caches,
  credentials, .git directory or untracked files. Preserve the checked-in lock,
  Cargo configuration, all runtime/build inputs, tests, docs, source verifiers
  and upstream licenses. Do not invent or grant a project license implicitly.
- Deterministic archive plus manifest records commit, version, compressed digest
  and exact per-file size/hash inventory. Repeating the export at one commit must
  produce identical bytes. Digests detect corruption; they are not signatures.
- Verification/extraction rejects malformed inventories, duplicate/unsafe paths,
  links and special entries, mismatched hashes, excessive file count/size/total
  uncompressed bytes. Write only into a new owned directory after validation;
  never overwrite an existing tree or trust tar extractall on external data.
- Test relocated source with the same external checker, verify all five SDK
  diffs and perform offline compilation/run. Do not rely on the original
  checkout or its root patch/config files. The registry cache may supply normal
  pinned third-party dependencies; no claim of fully vendored offline inputs.

## Plan and commands

1. Add the external graph regression and observe failure on the current tree.
2. Bind every hardened edge directly; refresh only required lock/diff metadata.
   Confirm exact package IDs/versions and run the external public-API fixture.
3. Implement source export/verification with small corruption/path tests; check
   deterministic output and run the relocated consumer. Record registry limits.
4. Run appropriate workspace regression, Clippy/fmt, schemas, source verifiers
   and audit. Check disk before/after builds; target budget 20 GiB. Clean finished
   alternate caches with cargo clean, keeping small logs/current binaries.

```
python tools/check_external.py
python tools/check_external.py --run
python tools/check_external.py --git --run
python -m unittest discover -s tools/tests -v
python tools/verify_grammers.py check
python tools/verify_rmcp.py check
python tools/source_bundle.py build
python tools/source_bundle.py check <archive> --manifest <manifest> --consumer
cargo test --locked --workspace --all-features --all-targets
cargo test --locked --workspace --all-features --doc
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cargo fmt --all -- --check
cargo audit
```

New tooling follows the stdlib-only Python scripts under tools; tests live in
tools/tests, the external Rust fixture under tools/fixtures/external-consumer. Runtime
fixtures use Rust assertions with no extra application framework. All Cargo
commands run from the source root, retaining the small-debug/no-incremental
repository profile, while the actual consumer workspace is a separate temp tree.

## Primary sources

- https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html#the-patch-section
- https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations
- https://doc.rust-lang.org/cargo/commands/cargo-package.html
- https://doc.rust-lang.org/cargo/reference/publishing.html
- https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure
- Pinned SDK manifests and existing source verifiers define the hardened edge
  inventory; grammers-crypto has no dependency on the patched TL/session crates.

## Increment status

Dependency binding and external checker are implemented. Path acceptance passed:
three libraries and five exact SDKs with no chrono; public fixture exercises
message/callback/upload/Unknown, MCP discovery, MT real-TL and joined close.
The old registry graph failed before the fix. Manifest regressions also reject
path+version fallback on our own library edges. Workspace: 681 all-feature tests,
16 doctests, clean workspace/isolated production Clippy; 30 Python checks and all
five source verifiers. Session20/sender16 tests validate the standalone path
graph. Root lock versions are unchanged; audit337 is clean. Pinned Git runtime
acceptance also passed on committed snapshot 8f42a0efc2db57fb9b70294ec2239488d2b53e27:
all eight protected nodes share the exact Git source/rev and expected paths,
with no chrono or consumer patch rules. It ran the same public fixture after
offline locked compilation; initial local Git metadata refreshed the public
registry index. Audit the external lock separately because its broad registry
requirements may resolve newer compatible versions than the workspace lock.
Source archive stage and registry packaging remain unimplemented; those are
still required release work. Fresh implementation review's internal-library
fallback gap was fixed, regression covered and scoped re-review is clear.
Cross-model skipped: autonomous continuation.

The pinned Git consumer's independently resolved lock was also audited: 278
packages, zero vulnerabilities/warnings. Evidence now lives in a sibling
*-evidence directory so Cargo itself creates/tags the build directory. This
fixes an observed native cleanup refusal (missing CACHEDIR.TAG); metadata-only
checking leaves the Cargo target absent, and a tiny real compile/clean proof
passes with the corrected layout. Completed standalone and external caches are
cleaned after their processes finish; compact workspace cache remains reusable.
