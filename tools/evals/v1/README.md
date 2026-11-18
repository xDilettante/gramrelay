# Rust API usability evaluation v1

The frozen corpus in tasks.json contains 30 prompts with behavioral fixtures
and reference solutions. The score gate is 24 of 30 first submissions. The
ten MCP retrieval questions remain a separate evaluation.

Each implemented case has a public function signature in signatures.json,
a reference implementation under reference/, and a Rust test under checks/.
The test imports the independently supplied function as solution::solve. Give
solvers only the task prompt, public signature and the specified documentation
packet. The reference and test inputs stay with the evaluator.

```powershell
python tools/eval_coding.py packet T01
python tools/eval_coding.py run --reference --toolchain 1.100.0
```

The first command prints a standalone Markdown packet and the second checks
the available reference cases. Each run keeps a unique receipt, exact source
and packet hashes, checked dependency lock, answer bytes, compiler command,
and separate build and test logs under target/eval-coding-evidence.
Use a clean commit so the receipt identifies the library and checks precisely.
The target can be cleaned via Cargo after the process exits.

For a first-check run, give each fresh agent exactly its task packet without
access to the repository, the test, previous answers or a compiler. Record the
model label the platform exposes. Save the first Rust module verbatim in T01.rs,
T02.rs and so on before reading the test result. Treat an absent answer as a
failed attempt. Keep the saved answers and the receipt when testing a repair.

Create provenance.json with schema_version 1, model, source_commit, protocol
`fresh-context-docs-only`, and an attempts object with keys T01 through T30.
Each attempt records its distinct session identifier, packet_sha256, answer_sha256
(null if unavailable) and contract_review (pass or fail for valid answers).
For unavailable bytes, record disposition as missing or invalid-answer. Keep
invalid originals separately if available. A failed manual review disqualifies
the answer before compilation or execution.
The manual review checks requirements that have no externally observable signal.
For example, the T09 implementation must build and discard a lazy request. A
function that does nothing would also leave the transport untouched. It also
checks that candidates stay within the task's authorized fixture interfaces.

```powershell
python tools/eval_coding.py run --submissions DIR --model NAME --provenance provenance.json --reference-receipt PATH_TO_REFERENCE_SUMMARY --toolchain 1.100.0
```

First-check requires a reference receipt with all 30 passes at the same archived
source, resolved lock and compiler. The runner checks both packet and answer hashes
before building. At least 24 passed cases satisfy the first-check target.
The runner invokes only the named evaluator test. Candidate tests remain
outside the score.
Use `--attempt repair` for a follow-up and `--attempt pilot` to exercise a draft
or a negative control without creating first-check evidence.
The runner executes Rust code locally. Give it only code from trusted or
reviewed inputs. Cases use synthetic data, MockTransport or loopback services.
Rust programs have the permissions of the operating-system account and must
avoid spawning processes or accessing source/evidence files.
