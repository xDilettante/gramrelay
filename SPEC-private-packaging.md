# Private Cargo package preparation (Q3)

## Objective

Prepare and verify the eight interdependent normalized Cargo archives locally,
without publishing, a real registry account or changes to working library/SDK
manifests. Existing source delivery remains supported. The private candidate
must preserve the authenticated SDK code and cannot resolve to public upstream
SDK packages. The user keeps original code closed; all original third-party
licenses/notices remain in effect. This is local preparation, not permission to
publish or a claim that an unknown production registry has been configured.

## Format and boundaries

- Input is a clean committed source ZIP verified by tools/source_bundle.py;
  materialize only its verified immutable files in an owned temporary tree.
- Stage all three libraries and five patched SDKs in one temporary workspace.
  Keep original Rust code, schemas, generators, features and licenses unchanged.
  Own versions stay alpha.1; SDK versions gain an explicit -tg.1 prerelease
  suffix. Every internal normal/dev/build edge has an exact version and the
  explicitly named local registry, alongside its staging path. No public SDK
  fallback, patch table or credentials. publish=false stays on all eight.
- Use cargo package --workspace --registry telegram-private --no-verify
  --exclude-lockfile --offline. These are library source archives; their consumer
  lock is the acceptance lock. Excluding per-package locks avoids resolving
  unrelated optional SDK features while assembling the mutually dependent
  archives. Do not treat --no-verify as successful build verification.
- Staging expands an SDK's restrictive include list to retain its existing
  license, PATCH.md and patch file, without editing those assets.
  Preserve the existing SDK Cargo.toml.orig bytes as Cargo.upstream.toml in
  staging: Cargo reserves the former name for its new generated staging manifest.
- Independently inspect each Cargo-generated normalized manifest and content:
  exact package name/version/publish/license, all internal registry edges,
  absence of dependency path/git sources and patch/replace/workspace inheritance,
  exact internal registry-index URLs, unchanged package
  source/assets, original staging manifest, and expected generated files only.
  Compare Cargo's complete staged/registry package target models (build script,
  library/binary/example/test targets, edition, feature gates and attributes),
  with source paths relative to each package. Keep declared/example/test assets.
  Cargo's omitted .gitignore/.gitattributes selection policies stay in the full
  source ZIP; this exception does not exclude Rust code or test fixture payloads.
- Prepare a minimal local Git registry index with file-URL downloads using the
  exact .crate checksums. No HTTP service, network publish API or token exists.
  The registry URL is tied to the candidate directory; these archives are local
  rehearsal artifacts. A future real registry needs explicit configuration and
  repackaging/acceptance; do not claim cross-directory archive reproducibility.
- Reuse tools/fixtures/external-consumer through an independent workspace with
  exact registry versions, no path/git dependencies, patch or source replacement.
  Bootstrap only the owned file registry with
  cargo info --registry telegram-private PACKAGE@VERSION for each exact package
  from a separate directory with no workspace manifest, then resolve and run offline. Verify
  exactly eight private registry nodes/versions/checksums, no duplicate hardened
  packages and no chrono; run the existing public Bot/MCP/MT fixture and joined
  close. All eight sources must be Cargo's registry copies, not staging files.
- Copy only checksum-verified public registry archives and their public sparse
  index records needed by the committed root lock into an isolated Cargo home.
  Never read/copy credential files or private registry configuration. Preserve
  the caller's installed rustup toolchain setup with auto-install disabled.
  Scratch ancestors must contain neither Cargo configuration nor Cargo.toml;
  check existence only and refuse if no neutral location is available.
  No implicit public network access.
- Keep small identity/commands/metadata/lock/runtime logs outside Cargo targets.
  Record failure before running each child; do not leave a successful result
  after failure. Source commit/archive hash and tool versions identify evidence.
  Enforce repository 20 GiB free/target guards, bounded archive reads and
  subprocess lifetimes. Clean terminal scratch build/cache data only.
  Keep Cargo's package-only output separate from a newly created runtime target:
  cargo package does not add CACHEDIR.TAG to its output, and reusing that directory
  prevents later cargo clean. For owned Windows Git cache files, retry deletion
  only on regular read-only files within the exact validated scratch tree.

## Implementation and commands

tools/private_packages.py uses Python 3.11+ stdlib, the existing source verifier,
Cargo packaging and Git. Reuse the existing external fixture and disk guard;
do not implement another transport, registry server, generic TOML writer or ZIP
extractor. Tests live in tools/tests/test_private_packages.py.

```
python -m unittest discover -s tools/tests -v
python tools/private_packages.py --toolchain 1.100.0
python tools/verify_grammers.py check
python tools/verify_rmcp.py check
python tools/schema.py check
cargo audit --no-fetch
```

Increment order: pure staging/index/content regression tests (observe RED),
minimal implementation, actual normalized package rehearsal, independent offline
runtime acceptance, fresh scoped review, documentation/evidence and commit.

Always retain source and third-party license bytes; use synthetic fixture data.
Never publish, push, contact Telegram or modify cryptographic implementation.
The offline audit uses the previously refreshed local advisory database; database
refresh is a separate verification action, outside this local-only rehearsal.
Actual registry choice, transfer/publication and final externally owned names
require a separate user instruction; continue other release gates meanwhile.

## Success criteria

All eight normalized archives and the independent registry consumer satisfy the
checks above. Record exact source identity, versions, checksums, graph and runtime
results. Main source graph/lock and SDK verifiers remain unchanged. Both macOS
cells and Q1/Q2/Q4 remain separate mandatory gates; full goal stays active.

## Recorded acceptance

Windows x86_64 MSVC / Rust 1.100.0 completed the full rehearsal from
b08bc736110fb9a5f548e81f0ffb9c9b15cbe022, source ZIP SHA256
ddd94ce13a1899b4c10e0684cb38e9aca0ca46887b78c208b94d9cf6b0323b64.
Evidence: target/private-packages/b08bc736110f-apn4__m9/evidence/summary.json;
19 commands, all exit0, eight verified normalized archives/private registry
nodes, complete staging/registry target models equal, independent lock278
without chrono. Consumer lock SHA256:
2b714952d936fd0386fd8b793875c3a7078b1599f4b46fecb883ad6b7912f181.
Bot message/callback/upload/Unknown, MCP discovery and MT real-TL/joined close
actually executed from registry copies. Root337 and consumer278 local-DB audits
are clean; pinned SDK source verification separately preserves their base-code
provenance rather than treating a private package identity as advisory coverage.

All 52 Python tests pass. Two pure-unit/runner review cycles and a third scoped
runner re-review are clear after canonical ordering, complete target models,
ancestor config/workspace isolation and failure-evidence fixes; cross-model
skipped autonomous. A separate narrow cleanup/target-separation review is also
clear. Actual eight-package execution exposed Cargo's reserved
Cargo.toml.orig, omitted ignore files and Windows read-only Git cleanup; each
has retained failure evidence and passing regression/actual acceptance.
The final run's Cargo clean removed 1.3 GiB, and its scratch no longer exists.
Runtime targets are separate from package-only output to preserve Cargo tagging.

An older completed run cd0f531448d8-i8zam5fe passed runtime but failed scratch
cleanup; its receipt remains failed. Deleting its untagged build directory via
verified PowerShell paths was rejected by automatic policy review (blocked by
policy); no alternate deletion was attempted. Its build and partial scratch are
retained, and the latest total target is 2.53 GiB with 427.17 GiB free.
This does not block the new successful rehearsal or erase the older failure.
