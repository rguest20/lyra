#!/usr/bin/env python3
"""Replay synthetic sources through Lyra's production grounding code.

Uses a fresh private database per sample and snapshots the executable. Successful
execution is not a semantic pass: replies and traces require separate review.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import time
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[1]
RUNTIME_FIELDS = {"question", "entities", "scope_constraints", "evidence", "inject_malformed"}


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def fixture_input(case: dict) -> dict:
    if set(case) - RUNTIME_FIELDS - {"id"}:
        raise ValueError("fixture cases cannot contain rubric or unknown fields")
    if not re.fullmatch(r"[a-z0-9_]+", case.get("id", "")):
        raise ValueError("case IDs must contain lowercase letters, numbers or underscores")
    if not isinstance(case.get("question"), str) or not case["question"].strip():
        raise ValueError("question must be nonempty")
    evidence = case.get("evidence")
    if not isinstance(evidence, list) or not 1 <= len(evidence) <= 20:
        raise ValueError("each case needs 1–20 source passages")
    ids = set()
    for passage in evidence:
        if not isinstance(passage, dict) or set(passage) != {"id", "origin", "passage"}:
            raise ValueError("passages allow only id, origin and passage")
        if not all(isinstance(value, str) and value.strip() for value in passage.values()):
            raise ValueError("passage fields must be nonempty strings")
        if passage["id"] in ids:
            raise ValueError("evidence IDs must be unique within a case")
        ids.add(passage["id"])
    for field in ("entities", "scope_constraints"):
        if not isinstance(case.get(field, []), list) or not all(isinstance(s, str) for s in case.get(field, [])):
            raise ValueError(f"{field} must be a string list")
    if type(case.get("inject_malformed", False)) is not bool:
        raise ValueError("inject_malformed must be boolean")
    return {key: case[key] for key in RUNTIME_FIELDS if key in case}


def audit_database(path: Path, expected: dict) -> dict:
    with sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True) as database:
        sources = database.execute("SELECT url, body FROM research_sources").fetchall()
        requests = database.execute("SELECT count(*) FROM host_request_log").fetchone()[0]
    expected_sources = {
        f"https://fixture.invalid/source-{index}": passage["passage"]
        for index, passage in enumerate(expected["evidence"], 1)
    }
    archive_complete = len(sources) == len(expected_sources) and dict(sources) == expected_sources
    return {"archived_sources": len(sources), "archive_complete": archive_complete,
            "http_requests": requests, "transport_valid": archive_complete and requests == 0}


def invoke(binary: Path, case: Path, environment: dict, timeout: float) -> dict:
    started = time.monotonic()
    timed_out = False
    with (case / "pipeline.stdout").open("w") as stdout, (case / "pipeline.stderr").open("w") as stderr:
        process = subprocess.Popen(
            [str(binary), "eval-fixture", str(case / "input.json"), str(case / "pipeline")],
            cwd=ROOT, env=environment, stdout=stdout, stderr=stderr, start_new_session=True,
        )
        try:
            process.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
            timed_out = isinstance(error, subprocess.TimeoutExpired)
            signal_group(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                signal_group(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
            if isinstance(error, KeyboardInterrupt):
                raise
    return {"exit_code": process.returncode, "timed_out": timed_out,
            "duration_seconds": round(time.monotonic() - started, 2)}


def signal_group(pid: int, sig: int) -> None:
    try:
        os.killpg(pid, sig)
    except ProcessLookupError:
        pass  # The process may finish between the timeout and signal.


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", type=Path, default=ROOT / "tests/fixtures/grounding-generalization.json")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/community-lab")
    parser.add_argument("--output", type=Path, required=True, help="new directory; existing runs are never overwritten")
    parser.add_argument("--case", action="append", dest="case_ids", help="case ID to run; repeat for several cases")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    try:
        if not 1 <= args.repeat <= 20 or args.timeout <= 0:
            raise ValueError("repeat must be 1–20 and timeout positive")
        raw = args.fixtures.read_bytes()
        fixtures = json.loads(raw)
        cases = fixtures["cases"]
        if not isinstance(cases, list) or not cases:
            raise ValueError("fixture set needs cases")
        payloads = {case["id"]: fixture_input(case) for case in cases}
        if len(payloads) != len(cases):
            raise ValueError("case IDs must be unique")
        if args.case_ids:
            if set(args.case_ids) - payloads.keys():
                raise ValueError("unknown requested case ID")
            payloads = {key: value for key, value in payloads.items() if key in args.case_ids}
        if not args.binary.is_file():
            raise ValueError("build the executable first with cargo build")
        args.output.mkdir(parents=True, exist_ok=False)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    directory = args.output.resolve()
    binary = directory / "runner-binary"
    shutil.copy2(args.binary, binary)
    write_json(directory / "fixtures.json", fixtures)
    manifest = {"fixture_set": fixtures.get("fixture_set"),
                "fixtures_sha256": hashlib.sha256(raw).hexdigest(),
                "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "started_at": datetime.now(timezone.utc).isoformat(), "completed": False,
                "training_gate_eligible": False, "semantic_adjudication": "not performed",
                "scope": "synthetic source transport; real extraction, persistence, retrieval and answer grounding",
                "samples": {}}
    write_json(directory / "run.json", manifest)
    environment = os.environ.copy()
    environment.update({"COMMUNITY_STATE": str(directory / "unused-agent.json"),
                        "COMMUNITY_KNOWLEDGE": str(directory / "unused-knowledge.json"),
                        "COMMUNITY_DB": str(directory / "unused-default.db"),
                        "COMMUNITY_CURMUDGEON_OPENAI": "0"})
    failed = False
    for repeat in range(1, args.repeat + 1):
        for case_id, payload in payloads.items():
            sample_id = f"{case_id}__{repeat}"
            case = directory / sample_id
            case.mkdir()
            write_json(case / "input.json", payload)
            print(f"[{len(manifest['samples']) + 1}/{len(payloads) * args.repeat}] {sample_id}", flush=True)
            result = invoke(binary, case, environment, args.timeout)
            try:
                observed = json.loads((case / "pipeline/observed.json").read_text())
                result["audit"] = audit_database(case / "pipeline/learning.db", payload)
                print(observed["answer_text"], flush=True)
            except (OSError, ValueError, sqlite3.Error) as error:
                result["audit_error"] = str(error)
            print(f"  {result['duration_seconds']}s exit={result['exit_code']} "
                  f"archive_complete={result.get('audit', {}).get('archive_complete', False)}", flush=True)
            failed |= result["exit_code"] != 0 or not result.get("audit", {}).get("transport_valid", False)
            write_json(case / "execution.json", result)
            manifest["samples"][sample_id] = result
            write_json(directory / "run.json", manifest)
    manifest["completed"] = True
    write_json(directory / "run.json", manifest)
    print("Execution and transport audited; semantic quality still requires review.", flush=True)
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
