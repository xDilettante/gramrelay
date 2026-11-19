# ADR 0023: include the pinned SDK comparison inputs

Status: accepted for local source distribution; fresh relocated acceptance is
recorded separately in SPEC-source-distribution.md.

Fresh Linux source checking failed because the original SDK .crate archives
were absent from Cargo cache. Normal builds resolve our patched SDK paths and
therefore never fetch those five original releases. The warmed Windows cache
had hidden this environment prerequisite; fresh CI and extracted source checks
must not depend on it.

Include the exact published four grammers 0.12.0 archives and rmcp 3.11.0 under
vendor/upstream: 947,749 bytes total. Their pinned SHA-256 values remain unchanged
and are authenticated before TAR parsing. SDK source/license bytes and Cargo
dependency selection are unchanged. These are immutable comparison inputs,
with their original licenses, inside the private source distribution.

Default lookup prefers the bundled regular file, then retains legacy Cargo/
download cache compatibility when it is absent. Corruption or an invalid bundled
file type fails without fallback. Explicit --archive remains an operator
override. check does not gain implicit network access; ordinary runtime
dependencies still need normal Cargo cache. The source package is not a fully
vendored offline dependency set.

Two CLI regressions failed before the change and now cover empty-cache/no-network
checking, corrupt input with a valid cache, invalid file type and explicit
override. All 43 tool tests pass on Windows and Linux; actual five-SDK checks
with empty CARGO_HOME pass. A fresh scoped review found no concrete issues.
Cross-model skipped: autonomous continuation.
