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
