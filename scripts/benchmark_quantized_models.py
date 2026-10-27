#!/usr/bin/env python3
"""Compare BF16 and INT8 checkpoints through Lyra's real `say` path."""

import argparse
import json
import os
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


MODELS = {
    "bf16": "models/qwen3.5-2b",
    "int8": "models/qwen3.5-2b-8bit",
}
PRECISIONS = ["bf16", "int8"]
PROMPT = "How can I prepare well for a job interview?"


def event_rows(stderr):
    rows = []
    for line in stderr.splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and "event" in value:
            rows.append(value)
    return rows


def summarize_files(root, output_dir, precision, repetition, return_code, wall_ms):
    run_dir = output_dir / f"{precision}-run-{repetition}"
    stdout_file = run_dir / "stdout.txt"
    stderr_file = run_dir / "stderr.jsonl"
    rows = event_rows(stderr_file.read_text())
    interaction_runs = []
    generations = []
    pending_generations = []
    for row in rows:
        if row.get("event") == "model.generation":
            pending_generations.append(row["details"])
        elif row.get("event") == "agent.inference":
            if row.get("details", {}).get("agent") == "interaction":
                interaction_runs.append(row["details"])
                generations.extend(pending_generations)
            pending_generations = []
    model_dir = MODELS[precision]
    loaded = [
        row["details"]
        for row in rows
        if row.get("event") == "model.loaded"
        and row.get("details", {}).get("model_dir")
        in {str(root / model_dir), model_dir}
    ]
    generation_thinking_tokens = sum(
        row.get("thinking_tokens", 0) for row in generations
    )
    budget_completion_tokens = sum(
        row.get("details", {}).get("thinking_tokens", 0)
        for row in rows
        if row.get("event") == "model.thinking_completed_at_budget"
    )
    thinking_tokens = max(generation_thinking_tokens, budget_completion_tokens)
    tokens = sum(row.get("generated_tokens", 0) for row in generations) + max(
        0, thinking_tokens - generation_thinking_tokens
    )
    inference_ms = sum(row.get("inference_ms", 0) for row in interaction_runs)
    summary = {
        "precision": precision,
        "repetition": repetition,
        "return_code": return_code,
        "wall_ms": wall_ms,
        "interaction_inference_ms": inference_ms,
        "generated_tokens": tokens,
        "thinking_tokens": thinking_tokens,
        "thinking_budget_completion_tokens": budget_completion_tokens,
        "generation_tokens_per_second": round(tokens * 1000 / inference_ms, 2)
        if inference_ms
        else None,
        "generation_attempts": len(generations),
        "generation_retries": max(0, len(generations) - 1),
        "peak_memory_bytes": max(
            (row.get("memory", {}).get("peak_bytes", 0) for row in generations),
            default=0,
        ),
        "loaded_model": loaded[-1] if loaded else None,
        "stdout_file": str(stdout_file.relative_to(root)),
        "stderr_file": str(stderr_file.relative_to(root)),
    }
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return summary


def run_one(root, output_dir, binary, mlx_python, precision, repetition, timeout):
    run_dir = output_dir / f"{precision}-run-{repetition}"
    run_dir.mkdir(parents=True, exist_ok=True)
    model_dir = MODELS[precision]
    role_adapters = {
        role: run_dir / f"{role.lower()}-adapter.safetensors"
        for role in ("SCOUT", "SCHOLAR", "CURMUDGEON", "ARCHIVIST", "INTERACTION")
    }
    env = os.environ.copy()
    env.update(
        {
            "COMMUNITY_MODEL_ID": "Qwen/Qwen3.5-2B",
            "COMMUNITY_MODEL_DIR": str(root / MODELS["bf16"]),
            "COMMUNITY_ADAPTER": str(run_dir / "default-adapter.safetensors"),
            "COMMUNITY_MLX_PYTHON": str(mlx_python),
            "COMMUNITY_CONTEXT_TOKENS": "2048",
            "COMMUNITY_MODEL_CACHE_SIZE": "2",
            "COMMUNITY_CURMUDGEON_OPENAI": "off",
            "COMMUNITY_DB": str(run_dir / "learning.db"),
            "COMMUNITY_KNOWLEDGE": str(run_dir / "knowledge.json"),
            "COMMUNITY_STATE": str(run_dir / "conversation.json"),
        }
    )
    for role, adapter in role_adapters.items():
        prefix = f"COMMUNITY_{role}"
        env[f"{prefix}_MODEL_ID"] = "Qwen/Qwen3.5-2B"
        env[f"{prefix}_MODEL_DIR"] = str(root / model_dir)
        env[f"{prefix}_ADAPTER"] = str(adapter)
    started = time.perf_counter()
    result = subprocess.run(
        [str(binary), "say", PROMPT],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    wall_ms = round((time.perf_counter() - started) * 1000)
    (run_dir / "stdout.txt").write_text(result.stdout)
    (run_dir / "stderr.jsonl").write_text(result.stderr)
    return summarize_files(
        root, output_dir, precision, repetition, result.returncode, wall_ms
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--analyze-only", action="store_true")
    parser.add_argument(
        "--output",
        default="evals/results/quantized-thinking-2026-10-24",
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    binary = root / "target/release/community-lab"
    mlx_python = root / ".venv-qwen35/bin/python"
    output_dir = root / args.output
    output_dir.mkdir(parents=True, exist_ok=True)
    if args.analyze_only:
        summaries = []
        for repetition in range(1, args.runs + 1):
            for precision in PRECISIONS:
                run_dir = output_dir / f"{precision}-run-{repetition}"
                prior = json.loads((run_dir / "summary.json").read_text())
                summaries.append(
                    summarize_files(
                        root,
                        output_dir,
                        precision,
                        repetition,
                        prior["return_code"],
                        prior["wall_ms"],
                    )
                )
        (output_dir / "summary.json").write_text(
            json.dumps(summaries, indent=2) + "\n"
        )
        print(json.dumps(summaries, indent=2))
        return
    (output_dir / "metadata.json").write_text(
        json.dumps(
            {
                "started_at": datetime.now(timezone.utc).isoformat(),
                "hardware": "Apple Silicon; see experiment document for details",
                "prompt": PROMPT,
                "thinking_budget": "Lyra defaults and extension policy",
                "repetitions": args.runs,
                "order": PRECISIONS,
                "note": "Every run assigns all five roles to one precision and has isolated state, database and role-specific adapters.",
            },
            indent=2,
        )
        + "\n"
    )
    summaries = []
    order = PRECISIONS
    for repetition in range(1, args.runs + 1):
        for precision in order:
            print(f"Running {precision}, repetition {repetition}...", flush=True)
            summary = run_one(
                root,
                output_dir,
                binary,
                mlx_python,
                precision,
                repetition,
                args.timeout,
            )
            summaries.append(summary)
            (output_dir / "summary.json").write_text(
                json.dumps(summaries, indent=2) + "\n"
            )
            print(json.dumps(summary), flush=True)
            if summary["return_code"] != 0:
                raise SystemExit(summary["return_code"])


if __name__ == "__main__":
    main()
