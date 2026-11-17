# Documentation and agent usability (Q1)

## Objective

Make the implemented alpha usable from short task-oriented documentation, then
measure the architecture's first-check target on 30 frozen Rust tasks: at least
24 compile and pass behavioral checks on the first submitted solution. Preserve
the full Bot API/runtime/agent/MTProto scope and the existing ten MCP archive
questions as a separate action/retrieval evaluation, not substitute those ten
for the 30 coding tasks. No score is inferred from reference solutions or tests.

## Deliverables and order

1. docs/START.md: one command+callback bot, offline self-check and explicit live
   command for a user with a prepared token; docs/AGENT-INDEX.md: compact current
   API/feature/task navigation; docs/RECIPES.md: select the working example and
   explain errors, lifecycle and permission choices. Update stale migration notes.
2. A shared command_callback example uses identical handlers/router in local
   MockTransport verification and polling. Default invocation is offline;
   --live is an explicit user action. Automated checks never choose live.
3. tools/evals/v1/tasks.json: 30 stable IDs/prompts and behavioral acceptance
   requirements spanning client, files/errors, runtime, journal/tools and MT.
   Establish executable fixtures and passing reference solutions before freezing
   an evaluation snapshot; keep solver prompts separate from reference/oracles.
4. A small stdlib runner records per-task first submitted code/hash, exact source
   and documentation identity, context budget, model provenance, compiler,
   command/exit/logs and behavioral result. A compile failure counts as a failed
   task; one broken task must not prevent evaluating the other 29. Never replace
   a submitted answer before its first-check result is retained. Repairs/new docs
   require a new labeled run and cannot overwrite the original score.
5. Fresh-context usability review executes the frozen prompts using only the
   selected documentation packet, with explicit tool/read restrictions. Record
   the actual exposed model identity; undisclosed provider revisions stay
   undisclosed. Measure coding and the separate MCP action evaluation honestly;
   a reference or parent-authored pilot is not the fresh-agent acceptance score.

## Public behavior and style

Use current examples/rustdoc as API reference; ARCHITECTURE.md is design history.
Required parameters go in constructors, optional values use builders and await.
For example, sent.message_id is the current scalar; do not reproduce the former
prototype's sent.id. Callback answer and application authorization are separate.
Unknown writes keep their operation key and never automatically resend.

No runtime/library API changes in the documentation slice. All examples use
synthetic in-memory/localhost tests. Credentials stay in the prepared host
environment, never source/config values, arguments, logs or evaluation artifacts.
Keep explicit feature flags and graceful owned-resource shutdown in recipes.

## Verification and commands

```
cargo test --locked --example command_callback --features test-kit
cargo run --locked --offline --example command_callback --features test-kit
cargo fmt --all -- --check
cargo clippy --locked --all-targets --features test-kit -- -D warnings
python -m unittest discover -s tools/tests -v
python tools/schema.py check
cargo audit
```

Use repository disk guards and compact profiles; isolate/clean terminal alternate
build targets, retain small logs and first-attempt artifacts. Do not retry deletion
of previously policy-blocked cache directories by another mechanism.

Docs smoke and first compilation prove code/example behavior. They do not prove
the architecture's new-human 15-minute live result; that requires a separately
authorized live onboarding observation. Keep unmeasured criteria explicit.
Both macOS cells, Q2 and Q4 remain required independent gates. Private licensing,
publish=false, source/SDK identity, migrations and permission semantics stay intact.
