"""Safety regressions for offline experiment preparation and C ABI cleanup."""
from collections import deque
from contextlib import nullcontext
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

import prepare_tdlib
import tdlib_probe


class ProbeTests(unittest.TestCase):
    def test_child_destination_outside_spike_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            work = root / "target/mtproto-spike"
            with patch.object(prepare_tdlib, "WORK", work):
                self.assertEqual(prepare_tdlib.contained(work / "downloads"), work / "downloads")
                with self.assertRaises(RuntimeError):
                    prepare_tdlib.contained(root / "outside")

    def test_existing_child_symlink_cannot_redirect_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            work = root / "target/mtproto-spike"
            with patch.object(prepare_tdlib, "WORK", work):
                outside = root / "outside"
                outside.mkdir()
                work.mkdir(parents=True)
                try:
                    (work / "dependencies").symlink_to(outside, target_is_directory=True)
                except OSError as error:
                    self.skipTest(f"directory symlink unavailable: {error.winerror if hasattr(error, 'winerror') else error.errno}")
                with self.assertRaises(RuntimeError):
                    prepare_tdlib.contained(work / "dependencies")

    def test_staged_or_untracked_source_inputs_cannot_be_reported_as_the_pinned_tree(self):
        for staged in [True, False]:
            with self.subTest(staged=staged), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                work = root / "target/mtproto-spike"
                source = work / "tdlib-source"
                source.mkdir(parents=True)

                def run(*args, env=None):
                    self.assertNotEqual(args[0], "cmake", "modified source must fail before configuration")
                    subprocess.run(args, check=True, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

                run("git", "init", str(source))
                (source / "CMakeLists.txt").write_text("# pinned\n", encoding="utf-8")
                run("git", "-C", str(source), "add", "CMakeLists.txt")
                run("git", "-C", str(source), "-c", "user.name=Offline", "-c", "user.email=offline@example.invalid", "commit", "-m", "fixture")
                commit = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
                if staged:
                    (source / "CMakeLists.txt").write_text("# staged alteration\n", encoding="utf-8")
                    run("git", "-C", str(source), "add", "CMakeLists.txt")
                else:
                    (source / "unexpected.cmake").write_text("# untracked input\n", encoding="utf-8")
                with patch.multiple(prepare_tdlib, ROOT=root, WORK=work, PACKAGES={}, TD_COMMIT=commit, run=run):
                    with self.assertRaises(subprocess.CalledProcessError if staged else RuntimeError):
                        prepare_tdlib.main()

    def test_nested_link_cannot_hide_in_gitignored_generated_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            work = root / "target/mtproto-spike"
            source = work / "tdlib-source"
            generated_parent = source / "td/generate"
            generated_parent.mkdir(parents=True)
            outside = root / "outside"
            outside.mkdir()
            try:
                (generated_parent / "auto").symlink_to(outside, target_is_directory=True)
            except OSError as error:
                self.skipTest(f"directory symlink unavailable: {error.errno}")
            with patch.object(prepare_tdlib, "WORK", work):
                self.assertEqual(prepare_tdlib.contained(source), source)
                with self.assertRaises(RuntimeError):
                    prepare_tdlib.verify_tree(source)

    def test_failed_abi_verification_still_closes_the_created_client(self):
        replies, sent = deque(), []

        def execute(raw):
            request = json.loads(raw)
            return json.dumps({"@type":"ok"} if request["@type"] == "setLogVerbosityLevel" else {"@type":"textEntities","entities":[{"offset":3,"length":19,"type":{"@type":"textEntityTypeUrl"}}]}).encode()

        def send(client, raw):
            request = json.loads(raw)
            sent.append(request["@type"])
            if request["@type"] == "close":
                replies.append({"@client_id":client,"@type":"updateAuthorizationState","authorization_state":{"@type":"authorizationStateClosed"}})
            else:
                replies.append({"@client_id":client,"@type":"authorizationStateReady","@extra":"offline-state"})

        library = SimpleNamespace(td_execute=MagicMock(side_effect=execute), td_create_client_id=MagicMock(return_value=1), td_send=MagicMock(side_effect=send), td_receive=MagicMock(side_effect=lambda _: json.dumps(replies.popleft()).encode() if replies else None))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            path = root / "target/mtproto-spike/tdlib-build/libtdjson.dll"
            path.parent.mkdir(parents=True)
            path.touch()
            with patch.object(tdlib_probe, "ROOT", root), patch.object(tdlib_probe.ctypes, "CDLL", return_value=library), patch.object(tdlib_probe.shutil, "which", return_value=str(root / "g++.exe")), patch.object(tdlib_probe.os, "add_dll_directory", side_effect=lambda _: nullcontext(), create=True):
                with self.assertRaises(AssertionError):
                    tdlib_probe.probe(path)
        self.assertEqual(sent, ["getAuthorizationState", "close"])


if __name__ == "__main__":
    unittest.main()
