"""Generate/check explicit Bot v1 or extended v2 schemas through the offline CLI."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("generate", "check"))
    parser.add_argument("--mtproto", action="store_true", help="extended inventory v2")
    args = parser.parse_args()
    version, count = (2, 16) if args.mtproto else (1, 8)
    result = subprocess.run(
        ["cargo", "run", "--locked", "--offline", "--quiet", "-p", "tg-agent",
         "--features", "cli,mtproto" if args.mtproto else "cli", "--bin", "tg-agent", "--", "schema"]
        + (["--mtproto"] if args.mtproto else []),
        cwd=ROOT, stdout=subprocess.PIPE, check=False,
    )
    if result.returncode:
        print("agent schema CLI failed", file=sys.stderr)
        return 1
    try:
        bundle = json.loads(result.stdout)
        if set(bundle) != {"schema_version", "request", "response", "catalog"}:
            raise ValueError("unexpected schema bundle")
        if (bundle["schema_version"] != version or bundle["catalog"]["schema_version"] != version
                or len(bundle["catalog"]["tools"]) != count):
            raise ValueError("unexpected schema version/inventory")
        artifacts = {
            name: (json.dumps(bundle[name], ensure_ascii=False, indent=2,
                              sort_keys=True, allow_nan=False) + "\n").encode("utf-8")
            for name in ("request", "response", "catalog")
        }
    except (ValueError, TypeError, KeyError):
        print("invalid agent schema bundle", file=sys.stderr)
        return 1
    directory = ROOT / "tg-agent" / "schemas" / f"v{version}"
    if args.command == "generate":
        directory.mkdir(parents=True, exist_ok=True)
    failed = False
    for name, expected in artifacts.items():
        path = directory / f"{name}.json"
        if args.command == "generate":
            path.write_bytes(expected)
        elif not path.is_file() or path.read_bytes() != expected:
            print(f"agent schema out of date: {path.relative_to(ROOT)}", file=sys.stderr)
            failed = True
    if failed:
        return 1
    print(f"{args.command}: 3 agent schema artifacts / {count} tools (v{version})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
