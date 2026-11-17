import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_compat as compat


class Compatibility(unittest.TestCase):
    def test_isolated_feature_coverage_tracks_manifest_additions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            optional = root / "implicit"
            (optional / "src").mkdir(parents=True)
            (optional / "src/lib.rs").write_text("")
            (optional / "Cargo.toml").write_text('[package]\nname="implicit"\nversion="0.1.0"\n')
            for relative, name in [(".", "tg"), ("tg-agent", "tg-agent"), ("tg-mtproto", "tg-mtproto")]:
                directory = root / relative
                directory.mkdir(exist_ok=True)
                (directory / "src").mkdir()
                (directory / "src/lib.rs").write_text("")
                dependency = "implicit" if relative == "." else "../implicit"
                workspace = '\n[workspace]\nmembers=["tg-agent","tg-mtproto"]\nexclude=["implicit"]\n' if relative == "." else ""
                (directory / "Cargo.toml").write_text(
                    f'[package]\nname="{name}"\nversion="0.1.0"\nrust-version="1.89"\n'
                    '[features]\ndefault=[]\nfirst=[]\nnew_feature=[]\n'
                    f'[dependencies]\nimplicit={{path="{dependency}",optional=true}}\n' + workspace, encoding="utf-8")
            toolchain = os.environ.get("RUSTUP_TOOLCHAIN", "stable")
            cases = compat.cases(root, toolchain, root / "build", "features")
            commands = [command for _, command in cases]
            for name in ("tg", "tg-agent", "tg-mtproto"):
                selected = [command for command in commands if command[command.index("--package") + 1] == name]
                for feature in ("first", "new_feature", "implicit"):
                    matches = [command for command in selected if "--features" in command
                               and command[command.index("--features") + 1] == feature]
                    self.assertEqual(len(matches), 1)
                self.assertTrue(any("--features" not in command and "--all-features" not in command for command in selected))
                self.assertTrue(any("--all-features" in command for command in selected))
                self.assertTrue(all("--lib" in command and "--no-default-features" in command and "--workspace" not in command for command in selected))
            self.assertTrue(all(command[:3] == ["cargo", f"+{toolchain}", "check"] and "--locked" in command for command in commands))

    def test_failure_replaces_old_success_and_stops_later_commands(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            evidence = root / "evidence"
            evidence.mkdir()
            (evidence / "summary.json").write_text('{"status":"passed"}')
            sentinel = root / "must-not-run"
            commands = [("ok", [sys.executable, "-c", "print('fixture')"]),
                        ("bad", [sys.executable, "-c", "raise SystemExit(7)"]),
                        ("later", [sys.executable, "-c", f"open({str(sentinel)!r},'w').close()"])]
            self.assertEqual(compat.execute(commands, evidence, {"suite": "fixture"}, root), 1)
            summary = json.loads((evidence / "summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual([case["exit_code"] for case in summary["cases"]], [0, 7])
            self.assertFalse(sentinel.exists())
            self.assertIn("fixture", (evidence / "01.log").read_text())

    def test_interruption_cannot_leave_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(compat.subprocess, "run", side_effect=KeyboardInterrupt), self.assertRaises(KeyboardInterrupt):
                compat.execute([("interrupted", ["cargo"])], root / "evidence", {}, root)
            summary = json.loads((root / "evidence/summary.json").read_bytes())
            self.assertEqual(summary["status"], "interrupted")
            self.assertEqual(summary["cases"][0]["command"], ["cargo"])
            self.assertIsNone(summary["cases"][0]["exit_code"])
            self.assertEqual(summary["cases"][0]["status"], "interrupted")
            self.assertEqual(summary["failure_type"], "KeyboardInterrupt")

    def test_concurrent_runs_have_independent_summary_and_log_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            def run(marker):
                evidence = compat.new_evidence(root / "build")
                command = [sys.executable, "-c", f"import time; time.sleep(.05); print({marker!r})"]
                self.assertEqual(compat.execute([("fixture", command)], evidence, {"marker": marker}, root), 0)
                return evidence
            with ThreadPoolExecutor(max_workers=2) as pool:
                directories = list(pool.map(run, ["A", "B"]))
            self.assertNotEqual(*directories)
            for directory in directories:
                summary = json.loads((directory / "summary.json").read_bytes())
                self.assertEqual(summary["status"], "passed")
                self.assertEqual((directory / "01.log").read_text().strip(), summary["marker"])

    def test_metadata_failure_keeps_context_exit_and_cargo_diagnostics(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for relative in compat.CRATES:
                directory = root / relative
                directory.mkdir(exist_ok=True)
                (directory / "Cargo.toml").write_text('[package]\nname="fixture"\nrust-version="1.89"\n')
            (root / "Cargo.lock").write_text("fixture")
            command = ["cargo", "+1.89.0", "metadata", "--locked", "--no-deps"]
            error = subprocess.CalledProcessError(101, command, output=b"", stderr=b"fixture Cargo resolution reason")
            with patch.object(compat, "ROOT", root), patch.object(sys, "argv", ["check_compat.py", "--toolchain", "1.89.0"]), \
                 patch.object(compat.subprocess, "check_output", side_effect=["rustc fixture version", error]), \
                 patch.object(compat.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, stdout="fixture", stderr="")):
                self.assertEqual(compat.main(), 1)
            summary_path, = (root / "target/compat-1.89.0-evidence").glob("run-*/summary.json")
            summary = json.loads(summary_path.read_bytes())
            self.assertEqual(summary["rustc"], "rustc fixture version")
            self.assertEqual(summary["preflight_error"]["command"], command)
            self.assertEqual(summary["preflight_error"]["exit_code"], 101)
            self.assertIn("fixture Cargo resolution reason", (summary_path.parent / summary["preflight_error"]["stderr_log"]).read_text())
