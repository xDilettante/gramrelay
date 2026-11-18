"""Run the locked HTTP/1.1 latency comparison or local admission workload."""
import hashlib
import json
import os
from pathlib import Path
import platform
from statistics import median
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from check_compat import guard

CLIENTS = ("tg", "teloxide", "frankenstein")
METHODS = ("getMe", "sendMessage", "answerCallbackQuery", "sendDocument")
RUST = "1.100.0"
TARGET = ROOT / "target/bench-http"


def compare(raw):
    count = raw["samples_per_batch"]
    if count != 1000 or raw["warmup_per_batch"] != 100 or raw["document_bytes"] != 65536:
        raise ValueError("unexpected benchmark configuration")
    p95 = {}
    for batch in raw["batches"]:
        key = batch["scenario"], batch["client"], batch["round"]
        durations = sorted(batch["samples_ns"])
        if (key in p95 or len(durations) != count or durations[0] <= 0
                or batch["p50_ns"] != durations[499] or batch["p95_ns"] != durations[949]):
            raise ValueError("incomplete or inconsistent latency samples")
        p95[key] = durations[949]
    if set(p95) != {(method, client, turn) for method in METHODS for client in CLIENTS
                    for turn in range(1, 4)}:
        raise ValueError("expected exactly one measurement per client, scenario and rotation")
    cases = []
    for method in METHODS:
        per_client = {client: [p95[method, client, turn] for turn in range(1, 4)] for client in CLIENTS}
        middle = {client: median(measurements) for client, measurements in per_client.items()}
        ratios = {client: middle["tg"] / middle[client] for client in CLIENTS[1:]}
        cases.append({"scenario": method, "p95_ns_by_round": per_client,
                      "median_p95_ns": middle, "tg_p95_ratios": ratios,
                      "within_20_percent": 5 * middle["tg"] <= 6 * min(middle[c] for c in CLIENTS[1:])})
    return cases


def read_command(*command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def run(pressure):
    if read_command("git", "status", "--porcelain"):
        raise ValueError("measure a clean source commit")
    commit = read_command("git", "rev-parse", "HEAD")
    lock = HERE / "Cargo.lock"
    lock_sha = hashlib.sha256(lock.read_bytes()).hexdigest()
    guard(ROOT)
    parent = ROOT / "target/bench-http-evidence"
    parent.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    print(f"evidence {evidence}", flush=True)
    name = "http-pressure" if pressure else "telegram-http-benchmark"
    build = ["cargo", f"+{RUST}", "build", "--locked", "--offline", "--release",
             "--manifest-path", str(HERE / "Cargo.toml"), "--target-dir", str(TARGET),
             "--bin", name]
    config = {"RUSTUP_AUTO_INSTALL": "0", "CARGO_BUILD_JOBS": "3", "CARGO_INCREMENTAL": "0",
              "CARGO_ENCODED_RUSTFLAGS": "", "RUSTC_WRAPPER": "", "RUSTC_WORKSPACE_WRAPPER": "",
              "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3", "CARGO_PROFILE_RELEASE_DEBUG": "0",
              "CARGO_PROFILE_RELEASE_LTO": "false", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
              "RUSTC": read_command("rustup", "which", "--toolchain", RUST, "rustc")}
    env = os.environ | config
    env.pop("CARGO_BUILD_TARGET", None)
    with (evidence / "build.log").open("wb") as log:
        subprocess.run(build, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    guard(ROOT)
    suffix = ".exe" if os.name == "nt" else ""
    binary = TARGET / "release" / (name + suffix)
    allowed = {"PATH", "SYSTEMROOT", "WINDIR", "TMP", "TEMP", "TMPDIR", "USERPROFILE", "HOME"}
    child_env = {k: v for k, v in env.items() if k.upper() in allowed}
    samples = evidence / "samples.json"
    with samples.open("wb") as stdout, (evidence / "run.log").open("wb") as stderr:
        subprocess.run([str(binary)], cwd=ROOT, env=child_env, stdout=stdout, stderr=stderr,
                       timeout=180, check=True)
    raw = json.loads(samples.read_bytes())
    if (read_command("git", "rev-parse", "HEAD") != commit or read_command("git", "status", "--porcelain")
            or hashlib.sha256(lock.read_bytes()).hexdigest() != lock_sha):
        raise ValueError("benchmark inputs changed during measurement")
    with binary.open("rb") as stream:
        binary_hash = hashlib.file_digest(stream, "sha256").hexdigest()
    summary = {"source_commit": commit,
               "rustc": read_command("rustc", f"+{RUST}", "-Vv"),
               "host": platform.platform(), "cpu": platform.processor(),
               "logical_cpus": os.cpu_count(), "build_command": build, "build_env": config,
               "binary_sha256": binary_hash,
               "lock_sha256": lock_sha}
    if pressure:
        summary.update(mode="http-admission", observations=raw)
    else:
        cases = compare(raw)
        summary.update(mode="latency", cases=cases,
                       meets_target=all(case["within_20_percent"] for case in cases))
    (evidence / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    if pressure:
        print(json.dumps(raw, indent=2))
        return 0
    for case in summary["cases"]:
        print(case["scenario"], case["median_p95_ns"], case["tg_p95_ratios"])
    return 0 if summary["meets_target"] else 1


if __name__ == "__main__":
    args = sys.argv[1:]
    if args not in ([], ["--pressure"]):
        raise SystemExit("usage: python tools/bench-http/run.py [--pressure]")
    raise SystemExit(run(pressure=bool(args)))
