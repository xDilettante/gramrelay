# ADR 0020: inherit hardened source dependencies

Status: accepted, 2026-11-17. Supersedes root-patch dependency binding in ADR0016
and ADR0019; their safety/lifecycle/formatting decisions remain unchanged.

An independent Cargo application selected all five original registry SDKs and
chrono because Cargo ignores patch tables in dependency workspaces. In-tree
tests alone did not establish the consumer's graph. Replace every internal
normal/dev edge with a relative path-only dependency, including SDK-to-SDK
edges; remove root and standalone patch tables. All three alpha libraries and
all five patched SDKs remain private with publish=false.

Do not use path+version as a shortcut: Cargo normalization drops paths from a
published manifest. That would silently restore unpatched registry packages.
Keep original SDK base versions/names and Rust library aliases for this source
distribution, with exact archive/diff verification. A registry-ready release
requires separately identified maintained patches or reviewed upstream fixes,
final package metadata/license and normalized .crate verification. No current
source-compatibility check is evidence that registry gate is complete.

The external checker validates every internal path-only edge, unique source/
version identity for all libraries/SDKs and no chrono in the entire external
lock. Its consumer has no patch table. Test-kit also compiles there. Public
fixtures exercise Bot message/callback/upload/Unknown, current MCP discovery,
real TL request/response and joined Client/Session close with no network.
Pinned Git consumption uses a local file URL and validates exact Git source/
commit plus sibling paths in Cargo's checkout; extracted archive verification
is the next source-distribution stage.

Source behavior is unchanged. cargo fmt --all now follows local path
dependencies; only existing local modifications in session message_box/mod.rs
needed whitespace formatting, covered by the recorded SDK diff.

See SPEC-source-distribution.md for acceptance commands and unfinished gates.
