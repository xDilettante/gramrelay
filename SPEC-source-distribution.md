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
- Include the five exact published SDK release archives under vendor/upstream,
  authenticated by the verifiers' pinned SHA-256 values. These small immutable
  comparison inputs are required for cache-independent offline patch checks;
  they are not selected as Cargo dependencies. Prefer the bundled file to Cargo
  or legacy download caches; a malformed/corrupt bundled input must fail, with no
  silent fallback. Explicit --archive remains an override. Runtime dependencies
  still use normal Cargo cache; this is not full dependency vendoring.
- Deterministic archive plus manifest records commit, version, compressed digest
  and exact per-file size/hash inventory. Repeating the export at one commit must
  produce identical bytes. Digests detect corruption; they are not signatures.
- Verification/extraction rejects malformed inventories, duplicate/unsafe paths,
  links and special entries, mismatched hashes, excessive file count/size/total
  uncompressed bytes. Write only into a new owned directory after validation;
  never overwrite an existing tree or use generic archive extraction on external data.
- Archive profile v1 uses native Git ZIP (store/deflate only), compressed 16 MiB,
  expanded files 64 MiB, individual file 16 MiB, 10,000 entries and 1 MiB JSON
  manifest. Preflight central-directory framing/count/name/extra limits before
  ZipFile allocates its member list. Reject ZIP64, encryption, multi-disk,
  unsupported modes and non-comment trailing data. Validate local headers against
  the directory, contiguous non-overlapping records and optional descriptors.
  Decode raw deflate with a bounded output and require its genuine end, exact
  size and CRC; ZipExtFile's advertised-size truncation is insufficient.
  All checks precede materialization. TAR/PAX is not a supported input format.
- Paths <=512 UTF-8 bytes, components <=255 bytes, depth <=16 including the
  prefix. Reject case/Unicode-normalization collisions, implicit parent aliases,
  Windows reserved/ambiguous names, file-as-parent conflicts and all links or
  special files. This profile excludes tilde-based 8.3 aliases and env/key files.
  Extract only verified regular file bytes into a new directory.
- Export native Git ZIP at explicit compression level 6 with replacement objects
  disabled, autocrlf=false, core.eol=lf, global attributes and fsmonitor disabled,
  automatic GC disabled, terminal prompts disabled and TZ=UTC. Status ignores
  submodules; the captured tree rejects gitlinks rather than inspecting helpers.
  Preserve the commit ZIP comment; source version/prefix and committed
  file inventory must agree. The manifest is derived from actual bytes, not
  working-tree timestamps or archive filenames.
  Reproducibility is checked twice on one commit with the same Git/Python tools.
- Capture the commit OID once; disable Git replacement objects on every read.
  Derive version and expected files from that OID's tree. Compare export paths,
  file modes and Git blob hashes independently (including export-ignore/subst
  injected through untracked info/attributes). Bound Git output/native child
  lifetime as well as archive data. Read archive/manifest once with bounded
  reads; materialize only the exact verified immutable byte snapshot.
- Plain check parses/verifies data only. --consumer is an explicit execution
  option for a trusted candidate built/reviewed by the operator: a checksum is
  not an authenticity proof and Cargo build scripts are executable code. The
  acceptance run here uses only our own committed candidate, extracted into an
  owned private temp directory. It invokes the relocated verifiers/checker, not
  code or Cargo config from the original source checkout.
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
- https://git-scm.com/docs/git-archive (commit timestamps/pax, tar.umask, attributes)
- https://docs.python.org/3/library/tarfile.html#extraction-filters (resource and extraction boundaries)
- https://docs.python.org/3/library/zipfile.html (native ZIP reader/CRC and metadata)
- https://docs.python.org/3/library/zlib.html#zlib.Decompress.decompress (bounded output and genuine stream termination)
- https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file (reserved names, streams, case and trailing characters)

The initial TAR/PAX proposal was replaced after fresh review identified parser
allocation/extension recursion before ordinary member limits. ZIP retains the
complete source-candidate objective with a smaller bounded metadata preflight.

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
Registry packaging remains required release work. Fresh implementation review's internal-library
fallback gap was fixed, regression covered and scoped re-review is clear.
Cross-model skipped: autonomous continuation.

The pinned Git consumer's independently resolved lock was also audited: 278
packages, zero vulnerabilities/warnings. Evidence now lives in a sibling
*-evidence directory so Cargo itself creates/tags the build directory. This
fixes an observed native cleanup refusal (missing CACHEDIR.TAG); metadata-only
checking leaves the Cargo target absent, and a tiny real compile/clean proof
passes with the corrected layout. Completed standalone and external caches are
cleaned after their processes finish. The user's disk-space request also led to
cleaning the completed root dev cache (6.2 GiB); current binaries and logs stay
separate. Full incremental/debug snapshots remain disabled.

The source ZIP tool and corruption/path/Git tests are implemented. Candidate
f754f79ac0fda1f14133f413f24ff01ac7796c71 exported twice with byte-identical ZIP
and manifest: 641 files, 2,335,089 compressed bytes, archive SHA-256
b61bebd94c494a15d78101da21fe6f27db8b03e65af6ebc3631dd86c41e756ed.
Plain check passed. Its relocated five-SDK verification and separate public
consumer compiled offline and passed Bot message/callback/upload/Unknown, MCP
discovery and MT real-TL/joined close; all eight protected graph nodes came from
the extracted tree. Registry cache availability is still required. Independently
resolved external lock: 278 packages, audit clean; root lock: 337, audit clean.
Evidence: target/source-bundle-consumer.log, target/source-consumer-evidence and
the two source-bundle audit logs. No production Rust behavior changed here.

All 35 Python tests pass, with deterministic protocol schema, five source
verifiers and cargo fmt clean. Review-driven RED-to-GREEN regressions cover local ZIP metadata,
underdeclared deflate output and normalize-after-casefold aliases. Configured
fsmonitor helpers are disabled with an invocation-sentinel regression. The
third scoped review found no remaining concrete failures; cross-model skipped
in this autonomous continuation. This is no claim of a general ZIP extractor.
The earlier 681 workspace tests/16 doctests remain the Rust regression baseline;
the new executable check specifically validates this archive's public API.
Source-candidate acceptance is complete locally; registry packaging, platform/
MSRV and Q1/Q2/Q4 remain open, so neither Q3 nor the full goal is complete.

User decision on 2026-11-17: original code remains closed. LICENSE and the three
own package license-file fields record that policy without a public grant;
publish=false and all upstream SDK licenses/provenance remain unchanged.
Registry preparation must preserve this private status; it is still a separate
local packaging gate and no registry endpoint is assumed. See
docs/PRIVATE-DISTRIBUTION.md. The later Windows/MSRV acceptance is recorded in
SPEC-compatibility.md; other native cells and the full goal remain open.

Fresh Linux checking exposed an original SDK-archive cache prerequisite hidden
by the warmed Windows environment. The selected Cargo graph uses patched SDK
paths, so it does not fetch their original public releases. Bundle the exact five
authenticated archives (under 1 MiB total) and make both verifiers prefer them.
This must also fix fresh CI/relocated checks without adding implicit network
access or changing upstream source/license bytes. Add cache-independent CLI,
corrupt-input and explicit-override regressions before implementation.

Implemented with 947,749 bytes of exact third-party input data, unchanged SDK
source/lock/license bytes and one shared archive locator. The two CLI
regressions failed before the fix; Windows43 and native Linux43 now pass, and
both actual verifiers succeed with an empty CARGO_HOME. A corrupt/non-regular
bundled input cannot silently use a valid cache; explicit --archive still works.
Fresh scoped review found no concrete issues; cross-model skipped autonomous.
Source candidate 767a65ee630cc727374af1f65dac53c6906cc741 was exported twice
identically: 656 files, ZIP SHA256
c43befb04455c7c145705fce4efc3e41b72d6768fbad201631b1a54fd0b53eae.
Native Linux checked the relocated archive's 43 Python tests, schema and all
five source verifiers, then executed its independent consumer offline with
ordinary public registry caches prepared. All eight hardened graph nodes
originate in the temporary extracted tree; Bot message/callback/upload/Unknown,
MCP discovery and MT real-TL/joined close passed. The independent 278-package
lock and root 337-package lock have clean audits; no chrono is present.
All 656 outer source inputs still match the ZIP. Evidence/receipt/lock are
retained in target/compat-linux-source-consumer-evidence and adjacent logs.
Cargo cleaned the completed 1.2 GiB consumer cache. This proves source delivery,
not registry normalization or the remaining release gates.
