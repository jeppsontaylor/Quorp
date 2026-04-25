from __future__ import annotations

import json
import os
import importlib.util
from importlib.machinery import SourceFileLoader
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from rust_swebench_five_agent_matrix_lib import (  # noqa: E402
    render_by_agent_markdown,
    render_by_problem_markdown,
    select_headline_attempt,
    summarize_cell,
)


RUNNER_PATH = SCRIPT_DIR / "quorp-rust-swebench-five-agent-matrix"
RUNNER_SPEC = importlib.util.spec_from_loader(
    "matrix_runner",
    SourceFileLoader("matrix_runner", str(RUNNER_PATH)),
)
assert RUNNER_SPEC is not None and RUNNER_SPEC.loader is not None
MATRIX_RUNNER = importlib.util.module_from_spec(RUNNER_SPEC)
RUNNER_SPEC.loader.exec_module(MATRIX_RUNNER)


def make_attempt(
    attempt: int,
    *,
    hidden_success: bool,
    visible_success: bool = False,
    status: str | None = None,
) -> dict:
    return {
        "attempt": attempt,
        "status": status or ("passed" if hidden_success else "failed"),
        "row": {
            "hidden_success": hidden_success,
            "visible_success": visible_success,
            "primary_failure": None if hidden_success else "visible_evaluation_failed",
            "wall_clock_ms": 100 * attempt,
            "total_tokens": 10 * attempt,
            "run_dir": f"/tmp/session-{attempt}",
            "result_path": f"/tmp/result-{attempt}.json",
            "summary_path": f"/tmp/summary-{attempt}.json",
            "session_id": f"session-{attempt}",
        },
    }


class MatrixSelectionTests(unittest.TestCase):
    def test_select_headline_attempt_prefers_first_hidden_pass(self) -> None:
        selected, reason = select_headline_attempt(
            [
                make_attempt(1, hidden_success=False),
                make_attempt(2, hidden_success=True),
                make_attempt(3, hidden_success=True),
            ]
        )
        self.assertIsNotNone(selected)
        self.assertEqual(selected["attempt"], 2)
        self.assertEqual(reason, "first_hidden_pass")

    def test_select_headline_attempt_falls_back_to_last_failure(self) -> None:
        selected, reason = select_headline_attempt(
            [
                make_attempt(1, hidden_success=False),
                make_attempt(2, hidden_success=False),
            ]
        )
        self.assertIsNotNone(selected)
        self.assertEqual(selected["attempt"], 2)
        self.assertEqual(reason, "last_failed_attempt")

    def test_render_views_include_all_cells(self) -> None:
        preflight = {
            "antigravity": {"passed": True},
            "cursor": {"passed": True},
            "claude": {"passed": True},
            "codex": {"passed": True},
            "quorp": {"passed": True},
        }
        cells = []
        for agent in ["antigravity", "cursor", "claude", "codex", "quorp"]:
            for case_id in ["case-a", "case-b"]:
                selected_attempt, selected_attempt_reason = select_headline_attempt(
                    [make_attempt(1, hidden_success=agent == "codex")]
                )
                cells.append(
                    summarize_cell(
                        agent=agent,
                        case_id=case_id,
                        attempts=[make_attempt(1, hidden_success=agent == "codex")],
                        selected_attempt=selected_attempt,
                        selected_attempt_reason=selected_attempt_reason,
                        preflight=preflight[agent],
                    )
                )

        report = {
            "output_dir": "/tmp/run",
            "attempts_per_cell": 3,
            "agent_order": ["antigravity", "cursor", "claude", "codex", "quorp"],
            "case_order": ["case-a", "case-b"],
            "preflight": preflight,
            "cells": cells,
        }

        by_agent = render_by_agent_markdown(report)
        by_problem = render_by_problem_markdown(report)
        for agent in report["agent_order"]:
            self.assertIn(f"## {agent}", by_agent)
        for case_id in report["case_order"]:
            self.assertIn(f"## {case_id}", by_problem)
            for agent in report["agent_order"]:
                self.assertIn(f"`{agent}`", by_problem)


class RunnerIntegrationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.home = self.root / "home"
        self.home.mkdir(parents=True, exist_ok=True)
        self.warpos_home = self.root / "moe" / "warpos"
        self.warpos_home.mkdir(parents=True, exist_ok=True)
        self.calls_log = self.root / "calls.jsonl"
        self.warpos_root = self.root / "warpos"
        self.suite_path = self.warpos_root / "challenges" / "suites" / "test.json"
        self._write_suite()
        self.fake_cli = self.root / "fake-warpos-cli.py"
        self._write_fake_cli()

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def _write_suite(self) -> None:
        case_root = self.warpos_root / "challenges" / "cases" / "case-a"
        (case_root / "workspace" / "proof-full").mkdir(parents=True, exist_ok=True)
        (case_root / "START_HERE.md").write_text("# Start\n", encoding="utf-8")
        (case_root / "evaluate.sh").write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
        self.suite_path.parent.mkdir(parents=True, exist_ok=True)
        self.suite_path.write_text(
            json.dumps(
                {
                    "id": "test-suite",
                    "suites": ["challenge-catalog"],
                    "challenges": ["case-a"],
                    "conditions": ["proof-full"],
                    "agents": ["antigravity", "cursor"],
                    "agent_order": ["antigravity", "cursor"],
                    "attempts_per_cell": 2,
                    "model": "ssd_moe/qwen36-27b",
                    "effort": "medium",
                }
            ),
            encoding="utf-8",
        )

    def _write_fake_cli(self) -> None:
        self.fake_cli.write_text(
            """#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import sys
from pathlib import Path


def value_after(args: list[str], flag: str) -> str:
    return args[args.index(flag) + 1]


args = sys.argv[1:]
calls_path = Path(os.environ["FAKE_WARPOS_CALLS"])
calls_path.parent.mkdir(parents=True, exist_ok=True)
with calls_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({
        "argv": args,
        "env": {
            "WARPOS_QUORP_MAX_ATTEMPTS": os.environ.get("WARPOS_QUORP_MAX_ATTEMPTS"),
        },
    }) + "\\n")

if args[:2] == ["auth", "verify"]:
    agent = value_after(args, "--agent")
    if agent == os.environ.get("FAKE_PREFLIGHT_FAIL_AGENT"):
        print(json.dumps({"agent": agent, "ok": False}))
        raise SystemExit(1)
    print(json.dumps({"agent": agent, "ok": True}))
    raise SystemExit(0)

if args[:2] == ["bench", "e2e"]:
    agent = value_after(args, "--agent")
    case_id = value_after(args, "--challenge")
    session_id = value_after(args, "--session-id")
    model = value_after(args, "--model") if "--model" in args else "fake-model"
    provider = value_after(args, "--provider") if "--provider" in args else "fake"
    attempt_token = session_id.split("-")[-2]
    success = attempt_token == "a002"
    warpos_home = Path(os.environ.get("WARPOS_HOME", str(Path(os.environ["HOME"]) / ".warpos"))).expanduser()
    session_root = warpos_home / "sessions" / session_id
    round_root = session_root / "benchmark-e2e" / "rounds" / "0001"
    preflight_root = session_root / "benchmark-e2e" / "preflight"
    round_root.mkdir(parents=True, exist_ok=True)
    preflight_root.mkdir(parents=True, exist_ok=True)
    (session_root / "benchmark-e2e").mkdir(parents=True, exist_ok=True)
    (session_root / "workspace").mkdir(parents=True, exist_ok=True)
    (session_root / "recorder" / "control").mkdir(parents=True, exist_ok=True)
    result = {
        "benchmark_status": "passed" if success else "failed",
        "visible_success": success,
        "hidden_success": success,
        "successful_turn_count": 1 if success else 0,
        "rounds_executed": 1,
        "failure_reason": None if success else "turn_failed",
        "issue": case_id,
        "condition": "proof-full",
        "suite": "challenge-catalog",
        "platform": agent,
        "provider": provider,
        "model": model,
        "run_id": session_id,
        "total_tokens": 42,
        "total_input": 30,
        "total_output": 12,
    }
    summary = {
        "benchmark_status": "candidate_pass" if success else "failed",
        "hidden_success": success,
        "visible_success": success,
        "runner_failure_kind": None if success else "turn_failed",
        "run_id": session_id,
        "suite": "challenge-catalog",
        "issue": case_id,
        "condition": "proof-full",
        "platform": agent,
        "started_at": "2026-04-19T00:00:00+00:00",
        "rounds_executed": 1,
        "successful_turn_count": 1 if success else 0,
    }
    (session_root / "benchmark-e2e" / "result.json").write_text(json.dumps(result), encoding="utf-8")
    (session_root / "summary.json").write_text(json.dumps(summary), encoding="utf-8")
    (round_root / "evaluation.json").write_text(json.dumps({"success": success}), encoding="utf-8")
    (round_root / "hidden-evaluation.json").write_text(json.dumps({"success": success}), encoding="utf-8")
    (round_root / "final-message.txt").write_text("final\\n", encoding="utf-8")
    (preflight_root / "final-message.txt").write_text("preflight\\n", encoding="utf-8")
    (session_root / "workspace" / "evaluation.json").write_text(json.dumps({"success": success}), encoding="utf-8")
    (session_root / "recorder" / "control" / "summary.json").write_text(json.dumps({"ok": True}), encoding="utf-8")
    print(json.dumps({"run_id": session_id}))
    raise SystemExit(0)

if args[:2] == ["telemetry", "export-dataset"]:
    session_id = value_after(args, "--session-id")
    output_dir = Path(value_after(args, "--output-dir"))
    fail_suffix = os.environ.get("FAKE_EXPORT_FAIL_SUFFIX", "")
    if fail_suffix and f"-{fail_suffix}-" in session_id:
        output_dir.mkdir(parents=True, exist_ok=True)
        raise SystemExit(2)
    (output_dir / "tables").mkdir(parents=True, exist_ok=True)
    (output_dir / "marts").mkdir(parents=True, exist_ok=True)
    raise SystemExit(0)

raise SystemExit(2)
""",
            encoding="utf-8",
        )
        self.fake_cli.chmod(0o755)

    def _run_runner(
        self,
        *,
        output_root: Path,
        resume: bool = False,
        preflight_fail_agent: str | None = None,
        export_fail_suffix: str | None = None,
        agents: list[str] | None = None,
        cases: list[str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        env = dict(os.environ)
        env["HOME"] = str(self.home)
        env["WARPOS_HOME"] = str(self.warpos_home)
        env["WARPOS_EXPECTED_HOME"] = str(self.warpos_home)
        env["FAKE_WARPOS_CALLS"] = str(self.calls_log)
        if preflight_fail_agent is not None:
            env["FAKE_PREFLIGHT_FAIL_AGENT"] = preflight_fail_agent
        else:
            env.pop("FAKE_PREFLIGHT_FAIL_AGENT", None)
        if export_fail_suffix is not None:
            env["FAKE_EXPORT_FAIL_SUFFIX"] = export_fail_suffix
        else:
            env.pop("FAKE_EXPORT_FAIL_SUFFIX", None)
        command = [
            sys.executable,
            str(RUNNER_PATH),
            "--warpos-root",
            str(self.warpos_root),
            "--suite",
            str(self.suite_path),
            "--warpos-cli",
            str(self.fake_cli),
            "--output-root",
            str(output_root),
            "--timeout-seconds",
            "60",
        ]
        if agents:
            command.extend(["--agents", *agents])
        if cases:
            command.extend(["--cases", *cases])
        if resume:
            command.append("--resume")
        return subprocess.run(
            command,
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )

    def _call_entries(self) -> list[dict[str, object]]:
        if not self.calls_log.exists():
            return []
        return [json.loads(line) for line in self.calls_log.read_text(encoding="utf-8").splitlines()]

    def test_preflight_failure_still_records_attempt(self) -> None:
        output_root = self.root / "report-preflight"
        completed = self._run_runner(
            output_root=output_root,
            preflight_fail_agent="cursor",
        )
        self.assertNotEqual(completed.returncode, 0)
        cursor_attempt = output_root / "cursor" / "case-a" / "attempt-001" / "attempt.json"
        self.assertTrue(cursor_attempt.exists())
        attempt_payload = json.loads(cursor_attempt.read_text(encoding="utf-8"))
        self.assertFalse(attempt_payload["preflight_passed"])
        self.assertTrue((output_root / "cursor" / "case-a" / "attempt-001" / "preflight" / "preflight.json").exists())
        bench_calls = [
            entry for entry in self._call_entries() if entry["argv"][:2] == ["bench", "e2e"]
        ]
        self.assertGreaterEqual(len(bench_calls), 2)

    def test_export_failure_keeps_real_result_row(self) -> None:
        output_root = self.root / "report-export"
        completed = self._run_runner(
            output_root=output_root,
            export_fail_suffix="a001",
        )
        self.assertEqual(completed.returncode, 0)
        attempt_payload = json.loads(
            (output_root / "antigravity" / "case-a" / "attempt-001" / "attempt.json").read_text(
                encoding="utf-8"
            )
        )
        row = attempt_payload["row"]
        self.assertEqual(attempt_payload["status"], "failed")
        self.assertNotEqual(attempt_payload["export_exit_code"], 0)
        self.assertEqual(row["source_type"], "warpos_benchmark_e2e_result_only")
        self.assertEqual(row["result_path"], attempt_payload["result_path"])

    def test_runner_passes_suite_model_effort_and_provider_to_warpos(self) -> None:
        output_root = self.root / "report-model"
        completed = self._run_runner(
            output_root=output_root,
            agents=["antigravity"],
            cases=["case-a"],
        )
        self.assertEqual(completed.returncode, 0)
        bench_calls = [
            entry for entry in self._call_entries() if entry["argv"][:2] == ["bench", "e2e"]
        ]
        self.assertGreaterEqual(len(bench_calls), 1)
        first_call = bench_calls[0]["argv"]
        self.assertEqual(first_call[first_call.index("--model") + 1], "ssd_moe/qwen36-27b")
        self.assertEqual(first_call[first_call.index("--effort") + 1], "medium")
        self.assertEqual(first_call[first_call.index("--provider") + 1], "local")

    def test_runner_builds_quorp_env_with_single_internal_attempt(self) -> None:
        environment = MATRIX_RUNNER.build_attempt_environment(
            "quorp", "ssd_moe/qwen36-27b"
        )
        self.assertIsNotNone(environment)
        assert environment is not None
        self.assertEqual(environment["WARPOS_QUORP_MAX_ATTEMPTS"], "1")

    def test_host_key_autoheal_detection_matches_stale_warpos_local_error(self) -> None:
        stderr_path = self.root / "warpos-e2e.stderr.log"
        stderr_path.write_text(
            "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n"
            "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n"
            "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n"
            "Host key verification failed.\n"
            "warpos-local\n",
            encoding="utf-8",
        )
        self.assertTrue(MATRIX_RUNNER.should_autoheal_warpos_local_host_key(stderr_path))

    def test_resume_skips_existing_attempts(self) -> None:
        output_root = self.root / "report-resume"
        first_completed = self._run_runner(output_root=output_root)
        self.assertEqual(first_completed.returncode, 0)
        initial_bench_calls = [
            entry for entry in self._call_entries() if entry["argv"][:2] == ["bench", "e2e"]
        ]
        self.assertEqual(len(initial_bench_calls), 4)

        second_completed = self._run_runner(output_root=output_root, resume=True)
        self.assertEqual(second_completed.returncode, 0)
        resumed_bench_calls = [
            entry for entry in self._call_entries() if entry["argv"][:2] == ["bench", "e2e"]
        ]
        self.assertEqual(len(resumed_bench_calls), 4)
        self.assertTrue((output_root / "antigravity" / "case-a" / "cell-summary.md").exists())

    def test_resume_archives_incomplete_attempt_before_rerun(self) -> None:
        output_root = self.root / "report-incomplete"
        output_root.mkdir(parents=True, exist_ok=True)
        attempt_dir = (
            output_root
            / "antigravity"
            / "case-a"
            / "attempt-001"
        )
        attempt_dir.mkdir(parents=True, exist_ok=True)
        (attempt_dir / "warpos-e2e.stdout.log").write_text("partial\n", encoding="utf-8")

        completed = self._run_runner(output_root=output_root, resume=True)
        self.assertEqual(completed.returncode, 0)
        archived = list((output_root / "antigravity" / "case-a" / "_incomplete_attempts").glob("attempt-001-*"))
        self.assertEqual(len(archived), 1)
        self.assertTrue((archived[0] / "warpos-e2e.stdout.log").exists())
        self.assertTrue((output_root / "antigravity" / "case-a" / "attempt-001" / "attempt.json").exists())

    def test_matrix_json_materializes_pending_cells_and_moe_provenance(self) -> None:
        output_root = self.warpos_home / "reports" / "report-pending"
        completed = self._run_runner(output_root=output_root, agents=["antigravity"])
        self.assertEqual(completed.returncode, 0)

        report = json.loads((output_root / "matrix.json").read_text(encoding="utf-8"))
        self.assertEqual(len(report["cells"]), 2)
        indexed = {(cell["agent"], cell["case_id"]): cell for cell in report["cells"]}

        antigravity_cell = indexed[("antigravity", "case-a")]
        self.assertEqual(antigravity_cell["status"], "passed")
        self.assertEqual(
            antigravity_cell["warpos_home_resolved"],
            str(self.warpos_home.resolve()),
        )
        self.assertTrue(antigravity_cell["moe_path_verified"])
        attempt_payload = json.loads(
            (
                output_root
                / "antigravity"
                / "case-a"
                / "attempt-002"
                / "attempt.json"
            ).read_text(encoding="utf-8")
        )
        self.assertEqual(
            attempt_payload["warpos_home_resolved"],
            str(self.warpos_home.resolve()),
        )
        self.assertTrue(attempt_payload["moe_path_verified"])
        self.assertTrue(
            attempt_payload["session_root_resolved"].startswith(str(self.warpos_home.resolve()))
        )

        cursor_cell = indexed[("cursor", "case-a")]
        self.assertEqual(cursor_cell["status"], "pending")
        self.assertEqual(cursor_cell["attempts_run"], 0)

    def test_resume_subset_keeps_full_matrix_and_updates_selected_scope(self) -> None:
        output_root = self.root / "report-resume-subset"
        first_completed = self._run_runner(output_root=output_root)
        self.assertEqual(first_completed.returncode, 0)

        completed = self._run_runner(
            output_root=output_root,
            resume=True,
            agents=["cursor"],
        )
        self.assertEqual(completed.returncode, 0)
        manifest = json.loads((output_root / "manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["agent_order"], ["antigravity", "cursor"])
        self.assertEqual(manifest["case_order"], ["case-a"])
        self.assertEqual(manifest["selected_agents"], ["cursor"])
        self.assertEqual(manifest["selected_cases"], ["case-a"])


if __name__ == "__main__":
    unittest.main()
