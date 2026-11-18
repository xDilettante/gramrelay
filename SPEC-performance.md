# Performance and load acceptance (Q2)

## HTTP comparison

Compare the production Bot API path with teloxide 0.17.0 and frankenstein
0.53.0. Pin the separately locked benchmark dependencies and keep them
outside the library workspace and package graph. Run a release binary on
one host with its own HTTP/1.1 loopback server and a synthetic credential.
Measure the duration of building each public request, sending it on a reused
connection and decoding its typed result. Use the same server, payloads and
goldens for all three clients.

Start with getMe, Unicode sendMessage, answerCallbackQuery and sendDocument
from the same 64 KiB local file. The server checks the method, content type,
logical request fields and multipart bytes, then returns a fixed valid
Telegram response. Require one successful result per measured request. The
server keeps no request history. Run the clients serially, in rotations of
client order, with independent warm-up for each client and scenario. Report
per-round sample counts, p50 and p95 and retain the raw samples. Compare the
median p95 with each competitor per scenario against the architecture's 20%
threshold. Keep release flags, compiler, resolved lock, source and host
configuration with the results. Large run-to-run variance warrants a rerun.

The adapters use the public HTTP clients. Teloxide uses reqwest 0.12 and
frankenstein uses reqwest 0.13, so this measures complete client paths. Use
explicit loopback URLs, no proxies or redirects, matching short timeouts,
HTTP/1.1 keepalive and a warmed file. Retain the libraries' production
validation and result handling. Keep Bot API and MTProto measurements
separate. Measure CPU, allocations and wire volume separately from latency.

## Saturation and memory

Exercise bounded HTTP admission with a server that holds admitted requests
open. Force demand above the configured cap, count structured overload
results, and verify the number of requests entering the server stays within
the cap. Release the held work, wait for it, and verify a subsequent request
is admitted. Capture bounded oversized response, streamed upload and
download behavior under pressure. Exercise webhook and agent queue limits
independently, observing counters, refused work and cancellation recovery.
Record the configured and observed peaks alongside a separate memory
measurement for saturated and settled workloads.

Follow the repository disk guard before each full build or run. Keep small
receipts and sample files adjacent to the build target. Clean the terminal
alternate Cargo target when validation ends.

## Reproduction

From the workspace root, fetch the benchmark's pinned public dependencies once.
Measure an idle host with the installed native Rust 1.100.0 toolchain.

```powershell
cargo +1.100.0 fetch --locked --manifest-path tools/bench-http/Cargo.toml
python tools/bench-http/run.py
```

A full run takes three rotations, 100 warm-ups per batch and 1,000 samples
per batch. The summary holds the median p95 for each client and method. The
raw nanosecond samples and Cargo evidence remain under
`target/bench-http-evidence`. A measured p95 above 120% of either competitor's
p95 marks that scenario as outside the target. The binary's `--smoke` mode
checks all requests and responses with six samples per client and method.

## Upstream API references

- [teloxide 0.17.0 HTTP client](https://docs.rs/teloxide/0.17.0/teloxide/struct.Bot.html)
- [frankenstein 0.53.0 asynchronous reqwest adapter](https://docs.rs/frankenstein/0.53.0/frankenstein/client_reqwest/)
