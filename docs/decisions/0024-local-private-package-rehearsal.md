# ADR 0024: rehearse normalized packages in a local private registry

Status: accepted for local preparation; acceptance evidence is recorded in
SPEC-private-packaging.md. Original code remains closed.

Our source-only path dependencies intentionally refuse Cargo packaging rather
than fall back to public, unpatched SDKs. Adding upstream versions would lose
that protection when Cargo removes dependency paths. A real registry/account
is not configured or authorized, but normalized archives still need verification.

Stage a verified clean source candidate, give the five SDKs explicit -tg.1
versions and bind every internal edge to exact private registry versions.
Leave working manifests and all runtime/schema/generator/license bytes unchanged.
Preserve upstream manifest bytes as Cargo.upstream.toml; Cargo reserves
Cargo.toml.orig for the generated staging manifest. Expand restrictive include
lists for existing source/license/patch assets. Keep publish=false everywhere.

Use Cargo's own package normalizer, then compare complete staged/registry Cargo
target models as well as dependency/features and included byte content. The
library archives omit per-package locks; an independent consumer lock, registry
checksums, offline build and public Bot/MCP/MT runtime are their acceptance.
No --no-verify success is interpreted as build success.

A minimal Git index and file:// downloads need no service or credentials.
Bootstrap exact private packages with cargo info from a manifest-free directory;
ordinary dependencies come from checksum-verified public root-lock cache inputs.
Scratch ancestor config/workspaces and inherited service/Cargo overrides are
excluded, rustup auto-install is disabled, and public HTTP is blocked locally.
The eight private nodes must be actual registry copies, with no source overrides.

Archive registry URLs refer to the owned candidate directory, so this is a local
rehearsal, not a portable production registry. A user-selected real registry
requires explicit configuration, repackaging and repeated acceptance. Publication,
transfer and remote push remain separately authorized actions. Full source ZIP
delivery remains the relocatable format and preserves all source/provenance.

Package-only output and compilation targets are separate: Cargo's package step
does not tag its target, so reusing it prevents safe cargo clean. Clean the
terminal tagged runtime cache with Cargo. Windows Git read-only regular pack
files are retried only within the exact owned scratch tree; uncertain child
termination retains data for inspection. Failure/cleanup evidence stays failed.

Primary contracts: [Cargo package](https://doc.rust-lang.org/cargo/commands/cargo-package.html),
[registry index](https://doc.rust-lang.org/cargo/reference/registry-index.html),
[Cargo configuration](https://doc.rust-lang.org/cargo/reference/config.html#hierarchical-structure),
[rustup environment](https://rust-lang.github.io/rustup/environment-variables.html),
[Python rmtree](https://docs.python.org/3/library/shutil.html#rmtree-example).
