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

No runtime/library API changes in the documentation slice. If corpus feasibility
exposes an explicit architecture capability missing from the public API, retain
that task and track a separately specified implementation prerequisite before
freezing; do not replace it with an easier supported task to improve the score.
All examples use
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

## Documentation increment evidence

START/RECIPES/AGENT-INDEX and the shared command_callback example are implemented.
Both negative controls failed with the stub router/mode selector; final example
passes two behavioral tests and its default actual offline run on Windows MSVC
with Rust 1.89.0 and 1.100.0. The body check includes exact target, reply parent,
text/full keyboard, callback ID/text, two requests and no unexpected data route.
Workspace default+tg/test-kit Clippy -D warnings, fmt, 16 workspace doctests,
52 Python tests, schema193/425 and fresh audit337 pass. Evidence: target/q1-*.log.
87 local documentation links and the complete 30-ID/six-area draft corpus pass
structural checks; the corpus SHA256 is
d02972399424e50b5de258f8573c51f785f0e55520ca2af25cb5ddb070992767.
Two scoped example review cycles and two docs/corpus contract cycles are clear
after strengthening body assertions, correcting path-only MT setup, boxed-object
vs union guidance and consumed-key reopening. Cross-model skipped autonomous.

T15 exposed a missing public AsyncRead upload entry point. Keep this required
one-shot-streaming task and its explicit prerequisite, rather than simplifying
the corpus. Its implementation belongs to a separate client/API slice before
fixture acceptance/freeze. Reference fixtures, 30-task fresh-agent first-check
score, ten-task MCP action score and new-human live timing remain unmeasured.
No actual Telegram library calls/live mode were used by these checks.

After all processes terminated, Cargo cleaned both alternate dev targets:
1.9 GiB current-stable and 701.3 MiB MSRV; logs are retained outside them.
Total target is 2.53 GiB, free C: 426.63 GiB. Previously policy-blocked old
directories were not touched. Q1 and the full goal remain active.

## Frozen coding corpus

The 30 tasks now have public signatures, executable behavioral checks and
passing reference modules. The T15 API prerequisite was completed separately
in 3ed62d1. Its evaluation check sends from a duplex reader while the producer
stays open and checks short input, limits and a single partial upload attempt.
T30 exercises driver cleanup and reopens the encrypted store before creating
a replacement client. The first T30 test incorrectly attempted to reuse the
closed Session directly. The corrected fixture tests the public lifecycle.

Commit 2c07b2b passed 30 of 30 reference cases on Windows MSVC Rust 1.100.0.
The later runner reads the completion footer of the single exact-name test,
so a candidate's stdout or extra tests do not change which oracle is scored.
The first-check command requires a new reference receipt at that same
archived snapshot and compiler. The runner records packets, submitted code,
hashes, source and lock identity and separate build and test results.

See tools/evals/v1/README.md for reproduction and provenance fields.

## First-check result

On Windows MSVC with Rust 1.100.0, gpt-6.2-sol passed 27 of 30 first
submissions against the 24-pass target. Snapshot 007673156579bb81a297fbc1fd3c9a832d1bb83e
and archive SHA256 9d9d19c7d9022d0184b46df3f6bf5712e95b14b2197149e6a347b672425c84d0
identify the exact library, fixtures and packets. All 30 references passed
at the same snapshot. Packet sizes ranged from 10,541 to 34,874 UTF-8 bytes
within the 49,152-byte limit. The 30 distinct fresh contexts received written
restrictions to read only their selected packets.

T03 selected the generated wire types for inline keyboard builders. T10
called an incorrect error accessor. T17 used an incorrect command-argument
accessor. Each failed at compilation. The next documentation snapshot now
names the top-level keyboard types and the exact `Error::outcome` and
`MessageContext::command_args` methods. The recorded attempts remain exact.

Evidence is in target/eval-coding-evidence/run-i76kcic4. This receipt includes
the passing reference receipt, all submitted modules, packet hashes, compiler
and fixture lock identity, and build and test logs. Negative control run
run-wju553ov distinguished compile failure, early exit without an oracle
result, a missing answer and an oversized answer from two real passes.
Cargo released 10.5 GiB after the evaluator processes finished. The separate
ten-question MCP evaluation and human onboarding timing are pending.
