# Maintained MCP timestamp dependency (Q3)

## Objective and scope

Remove chrono and RUSTSEC-2026-0543 from the entire locked workspace dependency
graph while retaining the official Rust MCP SDK, current/legacy wire behavior,
bounded CLI/MCP transport and owned executor lifecycle. Do not add an advisory
ignore, fake replacement package or second protocol implementation. Q3 platform,
MSRV and external-consumer packaging remain separate unfinished gates.

The official rmcp 3.11.0 release (17 November 2026, commit 165f8fd) still requires
chrono unconditionally and enables schemars chrono04. Use that maintained SDK
release as the pinned base, retain its upstream fixes, and record a minimal
source-verified downstream dependency patch. Jiff 0.2.46 is already in Cargo.lock
and is a RustSec-recommended alternative. Use its std-only UTC clock/formatter;
preserve the SDK wasm JS clock target feature without a timezone database.

## Contract

- Keep Annotations.last_modified as Option<String>. Preserve absent/null,
  date-only, RFC3339 and intentionally opaque/invalid incoming strings verbatim.
  The SDK convenience timestamp builders accept jiff::Timestamp; document this
  explicit SDK-only Rust type migration. tg-agent exposes no chrono timestamp
  argument, so its own public request/tool API stays unchanged.
- UTC timestamp helpers retain explicit +00:00 and the former AutoSi fractional
  precision (0/3/6/9 digits) for ordinary non-leap-second timestamps within
  RFC3339 years and Jiff's representable instant range. The latter is narrower
  than chrono, including timezone-offset headroom near calendar extrema.
  Jiff cannot represent typed leap seconds (its parser clamps
  :60 to :59); this is an explicit narrowing of the SDK convenience API, while
  raw lastModified strings including :60 remain verbatim. Delegate calendar/time
  formatting to Jiff. Task timestamps reuse the same helper; request-state
  expiry remains UTC epoch milliseconds, floored for negative fractions as in
  chrono. Use integer Euclidean division of Jiff's lossless epoch nanoseconds,
  with a checked conversion within Jiff's bounded range; no custom date engine.
- SDK timeout diagnostic uses std Duration Debug instead of chrono Duration.
  Tool errors retain their existing static/redacted codes/messages and never
  expose SDK diagnostic chains. No request/cancel/retry/driver behavior changes
  belong in the local patch; the upstream 3.11 changes are reviewed separately.
- Pin and hash-check the complete crates.io source archive. Keep all unmodified
  files byte-identical; allowlist only the dependency manifest, exact affected
  Rust/test files and a documented local helper/test. A deterministic recorded
  diff must detect unexpected changes. Retain upstream license/provenance.
- No chrono package remains in Cargo.lock or any workspace feature graph.
  Agent request schemas stay byte-identical. The v1/v2 output/catalog artifacts
  gain only a logically redundant root type:object for the existing object-only
  envelope variants. This is required by rmcp 3.11's legacy-peer SEP-2106 filter,
  which otherwise removes the schema from tools/list. Tool names, accepted
  requests/results and schema versions remain unchanged. Existing eight Bot and eight
  MT tools, both MCP lifecycle eras, stdio framing/duplicates/cancellation/EOF/
  broken output/Unknown restart and all Host acceptance must still pass.

## Implementation and evidence

1. Reproducible rmcp source import/verifier and official 3.11 pin; inspect its
   source delta and dependency closure before patching.
2. Timestamp helper, checked golden formatting/round-trip/epoch cases and SDK
   affected tests. Cover zero/milli/micro/nano fractions, negative epoch, leap
   day/year boundary, raw annotation preservation and live UTC millisecond scale.
3. Remove chrono/chrono04, resolve the lock without unrelated version drift;
   run SDK tests appropriate to changed model/task/expiry code and real adapter
   compatibility, then all workspace gates and fresh cargo audit.

Dependency-closure finding: Cargo issue rust-lang/cargo#10801 also retains the
disabled chrono integration of serde_with in Cargo.lock. grammers-session uses
that crate solely for Option<[u8;256]> hex serde. Replace that one adapter with
serde's with/default attributes and the already-locked hex 0.4.3 implementation,
then remove serde_with entirely. Preserve lowercase output, mixed-case input,
None/null/missing behavior and exact 256-byte decoded length. Malformed input
must return a static error without echoing key material. Test persisted snapshot
compatibility through the public DcOption contract before/after the change;
retain the pinned grammers archive verifier with an explicit types.rs allowlist.

Commands (root):

```
python tools/verify_rmcp.py check
python -m unittest discover -s tools/tests -v
cargo test --locked -p tg-agent --all-features --test mcp_timestamps
cargo test --locked --manifest-path vendor/rmcp/Cargo.toml --target-dir target/mcp-sdk --no-default-features --features server,client,macros,request-state --lib
cargo test --locked --manifest-path vendor/rmcp/Cargo.toml --target-dir target/mcp-sdk --no-default-features --features server,client,macros --test test_logging
cargo test --locked --workspace --all-features --all-targets
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --all-features --doc
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cargo fmt --all -- --check
python tools/agent_schema.py check
python tools/agent_schema.py check --mtproto
python tools/schema.py check
python tools/verify_grammers.py check
cargo audit
```

Always use synthetic data/local transports. No live Telegram, publishing, push or
new authority paths. The complete goal stays active after this dependency fix.

## Primary sources

- https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.11.0
- https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/rmcp-v3.11.0/crates/rmcp/Cargo.toml
- https://raw.githubusercontent.com/RustSec/advisory-db/main/crates/chrono/RUSTSEC-2026-0543.md
- Installed crates.io Jiff 0.2.46 src/fmt/temporal/mod.rs documents explicit UTC
  offset printing and precision; timestamp.rs defines epoch milliseconds. Exact
  installed source is used because the versioned docs.rs pages were unavailable.
- https://github.com/rust-lang/cargo/issues/10801 (disabled weak dependencies in lockfiles)
- Installed serde_with 3.24.0 src/hex.rs and Cargo.toml: exact existing hex
  adapter and weak chrono feature edge, checked before replacement.

Standalone SDK scope: its generated test lockfile still includes chrono through
the upstream optional OAuth2 auth feature. We neither expose nor enable that
feature, and retain upstream optional HTTP/auth code unchanged. The no-chrono /
clean-audit acceptance here applies to the entire Telegram workspace lockfile
and every workspace feature/target, not arbitrary standalone SDK feature sets.

## Local acceptance (17 November 2026)

- Telegram workspace: 681 all-feature /395 default tests, 16 doctests; clean
  workspace and isolated MCP/CLI Clippy, fmt; both schema inventories and pinned
  193-method/425-type Bot schema check. New consumer checks: four timestamp/
  timeout and four session-hex tests.
- SDK: 361 unit tests with explicit client/server/macros/request-state features;
  29 integration cases (logging5, MRTR13, structured-output3, tasks8). The original
  20 session SDK unit tests also pass with the new hex adapter.
- 28 Python tests and five pinned SDK verifications pass. The recorded rmcp diff
  includes the new executable helper; modifying that file fails verification.
- Numeric-key error has observed RED→GREEN proof; old hex golden cases passed
  before and after replacement. Legacy MCP discovery failed on missing output
  schema before the redundant object constraint and passes afterward. The four
  schema artifact deltas contain only 8/8/16/16 redundant object constraints;
  request artifacts are byte-identical.
- Fresh root audit: 337 packages, zero vulnerabilities and zero warnings. No
  unrelated registry version changes; no advisory ignores. Chrono/serde_with
  absent from the root lock, including every workspace feature/target closure.
- Stricter standalone SDK Clippy -D warnings reports 13 result_large_err issues
  in unchanged client/server initialization code. An authenticated unpatched
  3.11.0 baseline reports the exact same 13 paths/lines; scoped checking with
  only that inherited lint allowed passes. No SDK layout/API change or source
  lint suppression was added to address it. Workspace Clippy passes unmodified.
- Fresh read-only review findings were fixed and scoped re-review is clear.
  Cross-model skipped in this autonomous continuation. No live Telegram,
  publication, push, platform/MSRV or release-package claim. Q1–Q4 remain open.
