# Rust API usability evaluation v1

The corpus in tasks.json stays draft until every task has a passing reference
and a behavioral fixture. The score gate is 24 of 30 first submissions. The
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

For reviewed Rust submissions, use `run --submissions DIR --model NAME`.
Store the first submitted bytes in T01.rs, T02.rs and so on. Record the actual
model label and keep the original submission and receipt when testing a repair.
Use `--attempt repair` for a follow-up and `--attempt pilot` to exercise a draft
or a negative control without creating first-check evidence.
The runner executes Rust code locally. Give it only code from trusted or
reviewed inputs. Cases use synthetic data, MockTransport or loopback services.
Rust programs have the permissions of the operating-system account and must
avoid spawning processes or accessing source/evidence files.
