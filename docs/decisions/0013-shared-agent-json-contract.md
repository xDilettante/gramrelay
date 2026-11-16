# 0013: Shared machine contract before CLI/MCP adapters

Status: accepted for A2 increment 1. Standalone host config/execution and A3 MCP
remain open; discovery commands alone do not complete A2.

The tools module wraps A1 in a host-created ToolSession. Principal and Executor
never come from tool JSON. Host-only approve stores at most 256 exact-request
capabilities, retaining the executor's current revision/expiry checks. Named
adapters must bind their selected Action independently: execute_for rejects a
different Request.call.tool before quota or RPC. CLI selection comes from the
validated request; MCP must select its registered action independently of input.

ToolResponse gives a versioned JSON envelope around A1 results or dry-run data.
ok means an executor result exists; clients must inspect operation state and
availability. Error fields contain static codes/messages and intentional recovery
metadata. Validated write errors preserve the original key and get_operation
advice. Output IO/serialization failures conservatively retain Unknown and never
permit a new key. Debug/Display remain redacted; Telegram JSON is untrusted data.

Discovery is static and requires no credentials, config or journal. Each of eight
tools has strict input/argument and output schemas, effect/replay annotations and
examples. Canonical decimal-string IDs retain exact i64 limits, including signed
minimum, without JavaScript rounding. Schemas use draft 2020-12. Byte bounds,
duplicate-field detection, immutable references and policy remain runtime checks.
An independent jsonschema validator is a dev dependency with default external
resolution features disabled. It checks examples, ID boundaries, injections and
real responses from all eight actions. Deterministic artifacts in schemas/v1
come from the executable via tools/agent_schema.py, with a read-only check mode.

Embedded run_json and serve_jsonl use the same session. Frames are bounded before
dispatch; oversize closes rather than draining/resynchronizing arbitrary input.
Malformed bounded frames can continue. Serialized output is capped before writes,
and newline/flush failures preserve write recovery identity. No automatic retry
or detached execution path is introduced. A Broken-writer regression was observed
RED before the recovery-metadata fix and GREEN afterward. Focused reviews also
corrected selected-tool binding and write-denial example fidelity.

The first executable increment exposes tools/schema/help only. Trusted config
loading and standalone run/serve are the next increment, then actual process-level
execution tests. The full architecture goal remains unchanged.
