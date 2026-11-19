import json
import os
import subprocess
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_compat as compat


class Compatibility(unittest.TestCase):
    def test_relocated_source_never_inherits_git_identity_from_its_parent(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
        }):
            root = Path(temporary) / "репозиторий"
            root.mkdir()
            subprocess.run(["git", "init", "--quiet", "--template="], cwd=root, check=True)
            subprocess.run([
                "git", "-c", "user.name=Compatibility fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "--quiet", "--allow-empty", "-m", "Fixture",
            ], cwd=root, check=True)
            expected = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
            self.assertEqual(compat.git_context(root), (expected, False))
            extracted = root / "relocated source"
            extracted.mkdir()
            (extracted / "Cargo.toml").write_text("", encoding="utf-8")
            self.assertEqual(compat.git_context(extracted), (None, None))
            self.assertEqual(compat.git_context(root), (expected, True))

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
            # These tiny Python commands test evidence, not a Rust build's reserve.
            with patch.object(compat, "guard"):
                self.assertEqual(compat.execute(commands, evidence, {"suite": "fixture"}, root), 1)
            summary = json.loads((evidence / "summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual([case["exit_code"] for case in summary["cases"]], [0, 7])
            self.assertFalse(sentinel.exists())
            self.assertIn("fixture", (evidence / "01.log").read_text())

    def test_interruption_cannot_leave_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(compat, "guard"), patch.object(compat.subprocess, "run", side_effect=KeyboardInterrupt), self.assertRaises(KeyboardInterrupt):
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
            with patch.object(compat, "guard"), ThreadPoolExecutor(max_workers=2) as pool:
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
            (root / "Cargo.lock").write_text("version = 4\n")
            toolchain = os.environ.get("RUSTUP_TOOLCHAIN", "stable")
            command = ["cargo", f"+{toolchain}", "metadata", "--locked", "--no-deps", "--format-version", "1"]
            # Real malformed manifests avoid mocking subprocess globally: Unix
            # platform.platform() may itself invoke it before the rustc query.
            with patch.object(compat, "ROOT", root), patch.object(sys, "argv", ["check_compat.py", "--toolchain", toolchain]):
                self.assertEqual(compat.main(), 1)
            summary_path, = (root / f"target/compat-{toolchain}-evidence").glob("run-*/summary.json")
            summary = json.loads(summary_path.read_bytes())
            self.assertTrue(summary["rustc"].startswith("rustc "))
            self.assertEqual(summary["status"], "failed-preflight")
            self.assertEqual(summary["preflight_error"]["command"], command)
            self.assertEqual(summary["preflight_error"]["exit_code"], 101)
            self.assertIn("Cargo.toml", (summary_path.parent / summary["preflight_error"]["stderr_log"]).read_text())

    def test_real_guard_still_refuses_compilation_below_reserved_space(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(compat.shutil, "disk_usage") as usage, patch.object(compat.subprocess, "run") as launch:
                usage.return_value.free = 19 * 1024**3
                with self.assertRaisesRegex(ValueError, "20 GiB"):
                    compat.execute([("must-not-build", ["cargo"])], root / "evidence", {}, root)
                launch.assert_not_called()
            summary = json.loads((root / "evidence/summary.json").read_bytes())
            self.assertEqual(summary["status"], "interrupted")
            self.assertEqual(summary["failure_type"], "ValueError")
