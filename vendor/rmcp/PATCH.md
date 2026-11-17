# Maintained MCP timestamp dependency

Base: official rmcp 3.11.0, upstream commit
`165f8fd514afb8f6d1be3e86260ee9abfce58541` (`crates/rmcp`).
Archive: https://static.crates.io/crates/rmcp/rmcp-3.11.0.crate
SHA-256: `391f153431ab68664d946513ea0cb3edbca78eaf99c5a983036cacab6220d85b`.
The upstream LICENSE and .cargo_vcs_info.json remain byte-identical.

RUSTSEC-2026-0543 marks every chrono version unmaintained. This downstream
patch removes chrono and schemars' unused chrono04 integration, replacing the
SDK's clock/formatter with pinned Jiff 0.2.46. Native builds use std only; wasm
unknown targets retain a JS clock through Jiff's std/js features. No timezone
database, retry, dispatch, cancellation or transport changes are introduced.

## Rust API migration

`Annotations::for_resource` and `with_timestamp` now accept `jiff::Timestamp`,
re-exported as `rmcp::model::Timestamp`. Callers can parse an RFC3339 string or
construct that type without adding a direct Jiff dependency. tg-agent has no
public chrono arguments and its request/tool interfaces are unchanged.

The typed convenience helpers preserve chrono's explicit UTC `+00:00` and
0/3/6/9 fractional digits for ordinary non-leap-second RFC3339 timestamps in
Jiff's representable instant range (narrower than chrono, including offset
headroom near calendar extrema).
Jiff cannot represent leap seconds; its parser clamps `:60` to `:59`.
Applications needing opaque timestamps should set `last_modified` directly:
that Option<String> remains untouched, including invalid strings and `:60`.
Absent and null still both deserialize to None and serialize as omission.

Task metadata uses the same formatter. Request-state expiry remains epoch
milliseconds floored toward negative infinity, including negative fractional
milliseconds; Jiff's direct as_millisecond would truncate toward zero.
The SDK Timeout diagnostic now formats the full std::time::Duration with Debug,
instead of chrono's ISO duration. tg-agent still normalizes SDK failures into
its existing static/redacted tool errors.

## Verification and maintenance

Run `python tools/verify_rmcp.py check` from the workspace. The verifier checks
the authenticated archive, the exact source inventory, unchanged upstream bytes
and the deterministic diff in tg-timestamps.patch. Local helper source is
recorded as a diff against an empty base; this note is inventory-checked only.
Cargo.lock is generated for standalone SDK tests.
Run `generate` only after review of intentional source changes.

Test the SDK itself with server/client/macros/request-state enabled, plus its logging
integration test with macros enabled. Workspace tests alone do not enable the
dependency's request-state module or run its unit tests. Golden tests cover UTC
format/round-trip, precision, epoch-floor semantics and both timestamp extrema;
workspace consumer tests cover raw annotations, clocks and timeout diagnostics.
See SPEC-mcp-timestamps.md for the complete acceptance commands.

Root [patch] overrides are a development mechanism. External-consumer packaging
is an open Q3 gate; do not publish an artifact that silently loses this patch.
The standalone SDK lockfile still retains chrono through the optional upstream
OAuth2 auth feature. That feature is not enabled/exposed by Telegram Rust; its
entire workspace lockfile and enabled dependency closure are chrono-free.
