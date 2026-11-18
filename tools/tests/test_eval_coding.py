import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import eval_coding as evaluation


class CodingEvaluation(unittest.TestCase):
    def test_all_packets_use_public_signature_within_budget(self):
        suite, tasks, signatures = evaluation.inventory(evaluation.ROOT)
        self.assertEqual(set(signatures), set(tasks))
        for task_id, task in tasks.items():
            with self.subTest(task=task_id):
                raw = evaluation.packet(evaluation.ROOT, suite, task, signatures[task_id])
                self.assertLessEqual(len(raw), suite["controlled_context_max_utf8_bytes"])
                document = raw.decode()
                self.assertIn(task["prompt"], document)
                self.assertIn(signatures[task_id], document)
                self.assertNotIn("include!(\"check.rs\")", document)
                self.assertNotIn("Оценка:", document)

    def test_vetted_toolchain_and_prompt_and_answer_identity_match(self):
        ids = [f"T{n:02}" for n in range(1, 31)]
        cases = [{"id": task, "packet_sha256": "packet-" + task, "answer_sha256": "answer-" + task}
                 for task in ids]
        summary = dict(model="reported-model", source_commit="commit", archive_sha256="archive",
                       rustc="compiler", lock_sha256="lock", cases=cases)
        reference = dict(archive_sha256="archive", rustc="compiler", lock_sha256="lock",
                         mode="reference", status="finished", passed=30,
                         cases=[case | {"status": "passed"} for case in cases])
        data = dict(schema_version=1, model="reported-model", source_commit="commit",
                    protocol="fresh-context-docs-only",
                    attempts={case["id"]: dict(session="session-" + case["id"], contract_review="pass",
                                               packet_sha256=case["packet_sha256"],
                                               answer_sha256=case["answer_sha256"]) for case in cases})
        evaluation.check_provenance(data, reference, summary, ids)
        for field in ("packet_sha256", "answer_sha256", "session", "contract_review"):
            wrong = copy.deepcopy(data)
            wrong["attempts"]["T02"][field] = wrong["attempts"]["T01"][field] if field != "contract_review" else "missing"
            with self.subTest(field=field), self.assertRaises(ValueError):
                evaluation.check_provenance(wrong, reference, summary, ids)
        for field in ("passed", "archive_sha256", "rustc", "lock_sha256"):
            with self.subTest(field=field), self.assertRaises(ValueError):
                evaluation.check_provenance(data, reference | {field: None}, summary, ids)

    def test_executable_must_match_the_task_inside_owned_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = root / "target"
            target.mkdir()
            binary = target / "t01.exe"
            binary.touch()
            log = root / "cargo.jsonl"
            event = dict(reason="compiler-artifact", target=dict(kind=["test"], name="t01"),
                         executable=str(binary))
            log.write_text(json.dumps(event))
            self.assertEqual(evaluation.executable(log, target, "t01"), binary)
            with self.assertRaises(ValueError):
                evaluation.executable(log, target, "t02")
            outside = root / "outside.exe"
            outside.touch()
            log.write_text(json.dumps(event | {"executable": str(outside)}))
            with self.assertRaises(ValueError):
                evaluation.executable(log, target, "t01")
