# ADR 0019: maintained MCP clock and explicit object outputs

Status: accepted, 2026-11-17. Extends ADR 0015.

RustSec marks all chrono releases unmaintained (RUSTSEC-2026-0543). The official
rmcp 3.11.0 release still requires chrono. Retain that SDK as a hash-verified
vendored base, with a small recorded Jiff 0.2.46 patch; do not ignore the advisory,
pretend a different package is chrono or implement another MCP protocol stack.
The verifier authenticates the release archive, preserves 188 upstream files and
records changes to eight files plus one new helper. Upstream license/provenance
are unchanged.

Annotations keep opaque Option<String> wire values. Typed convenience builders
explicitly migrate to rmcp::model::Timestamp (Jiff), whose range is narrower and
does not represent leap seconds. Ordinary representable RFC3339 instants retain
UTC +00:00 and 0/3/6/9 fractional precision. Task metadata reuses that formatter;
request-state milliseconds preserve negative floor semantics. SDK timeout text
uses std Duration Debug, while tg-agent keeps static errors with no SDK chains.

Cargo also retained the disabled chrono integration of serde_with because of
Cargo issue10801. grammers-session's only use was one optional 256-byte key hex
adapter. Replace it with serde attributes and existing hex 0.4.3, preserving
snapshot bytes, lowercase/mixed-case semantics and None/null/missing behavior.
The decoder writes into a fixed array and emits a static error for malformed
strings/types. Golden tests passed before and after; the non-string diagnostic
regression has observed RED→GREEN evidence. No crypto or encrypted snapshot
version changes are made.

The upstream 3.11 legacy filter removes outputSchema without root type:object.
Our response schemas previously expressed object-only variants via oneOf. Add
the logically redundant object constraint to their common producer, regenerate
v1/v2 catalogs/response schemas and retain identical request schemas. Existing
MCP discovery comparison failed on missing outputSchema before this adjustment.
Both current and legacy protocol paths must continue exporting the shared
catalog. No tool names, accepted payloads or schema versions change.

The clean audit requirement covers the entire Telegram workspace lockfile and
every workspace feature/target closure. The SDK's standalone test lockfile still
includes chrono through its optional OAuth2 auth feature, which Telegram Rust
does not expose/enable. Optional upstream HTTP/auth code is retained untouched.
Root patch overrides also remain a development mechanism; external-consumer
packaging and platform/MSRV validation are separate open Q3 release gates.

See SPEC-mcp-timestamps.md for sources, commands and acceptance evidence.
