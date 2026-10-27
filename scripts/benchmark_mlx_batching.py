#!/usr/bin/env python3
"""Compare serial and same-model MLX-LM batching for independent prompts."""

import argparse
import json
import statistics
import time
from importlib.metadata import version
from pathlib import Path

import mlx.core as mx
from mlx_lm.generate import BatchGenerator, generate_step
from mlx_lm.sample_utils import make_sampler
from mlx_lm.utils import load_model
from tokenizers import Tokenizer


PROMPTS = [
    "Give one practical tip for preparing for a job interview.",
    "Explain photosynthesis in one clear sentence.",
    "Name one reason to keep a software backup.",
    "What is one simple way to reduce household energy use?",
]
MAX_TOKENS = 64


def generate_serial(model, prompts, sampler, stop_ids, tokenizer):
    started = time.perf_counter()
    results = []
    for prompt in prompts:
        prompt_started = time.perf_counter()
        tokens = []
        generator = generate_step(
            mx.array(prompt),
            model,
            max_tokens=MAX_TOKENS,
            sampler=sampler,
            prefill_step_size=128,
        )
        try:
            for token, _ in generator:
                token = int(token.item()) if hasattr(token, "item") else int(token)
                if token in stop_ids:
                    break
                tokens.append(token)
        finally:
            generator.close()
        results.append(
            {
                "elapsed_ms": round((time.perf_counter() - prompt_started) * 1000),
                "token_ids": tokens,
                "text": tokenizer.decode(tokens),
            }
        )
        mx.clear_cache()
    return round((time.perf_counter() - started) * 1000), results


def generate_batched(model, prompts, sampler, stop_ids, tokenizer):
    started = time.perf_counter()
    generator = BatchGenerator(
        model,
        max_tokens=MAX_TOKENS,
        stop_tokens=[[token] for token in stop_ids],
        sampler=sampler,
        prefill_batch_size=len(prompts),
        completion_batch_size=len(prompts),
        prefill_step_size=128,
    )
    tokens_by_id = {}
    elapsed_by_id = {}
    try:
        uids = generator.insert(prompts)
        tokens_by_id = {uid: [] for uid in uids}
        while len(elapsed_by_id) < len(uids):
            for response in generator.next_generated():
                if not (
                    response.finish_reason is not None
                    and response.token in stop_ids
                ):
                    tokens_by_id[response.uid].append(response.token)
                if response.finish_reason is not None:
                    elapsed_by_id[response.uid] = round(
                        (time.perf_counter() - started) * 1000
                    )
    finally:
        generator.close()
    results = [
        {
            "elapsed_ms": elapsed_by_id[uid],
            "token_ids": tokens_by_id[uid],
            "text": tokenizer.decode(tokens_by_id[uid]),
        }
        for uid in uids
    ]
    return round((time.perf_counter() - started) * 1000), results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, default=Path("models/qwen3.5-2b-8bit"))
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("evals/results/mlx-batching-2026-10-25/results.json"),
    )
    parser.add_argument("--repetitions", type=int, default=4)
    args = parser.parse_args()

    model_dir = args.model.resolve()
    model, config = load_model(model_dir, strict=True)
    model.eval()
    model.freeze()
    mx.eval(model.parameters())
    tokenizer = Tokenizer.from_file(str(model_dir / "tokenizer.json"))
    stop_ids = {
        tokenizer.token_to_id("<|im_end|>"),
        tokenizer.token_to_id("<|endoftext|>"),
    }
    if None in stop_ids:
        raise RuntimeError("model tokenizer is missing a stop token")
    prompts = [
        tokenizer.encode(
            f"<|im_start|>user\n{question}<|im_end|>\n"
            "<|im_start|>assistant\n<think>\n\n</think>\n\n"
        ).ids
        for question in PROMPTS
    ]
    sampler = make_sampler(temp=0.1, top_p=0.8, top_k=20)
    records = []
    for repetition in range(args.repetitions):
        modes = (
            [("serial", generate_serial), ("batch", generate_batched)]
            if repetition % 2 == 0
            else [("batch", generate_batched), ("serial", generate_serial)]
        )
        for mode, generate in modes:
            elapsed, results = generate(model, prompts, sampler, stop_ids, tokenizer)
            record = {
                "repetition": repetition + 1,
                "mode": mode,
                "elapsed_ms": elapsed,
                "per_prompt_elapsed_ms": [item["elapsed_ms"] for item in results],
                "generated_tokens": [len(item["token_ids"]) for item in results],
                "results": results,
            }
            records.append(record)
            print(json.dumps({key: value for key, value in record.items() if key != "results"}), flush=True)
            mx.clear_cache()

    measured = [record for record in records if record["repetition"] > 1]
    summary = {
        mode: {
            "median_group_elapsed_ms": round(
                statistics.median(
                    record["elapsed_ms"]
                    for record in measured
                    if record["mode"] == mode
                )
            ),
            "median_last_prompt_elapsed_ms": round(
                statistics.median(
                    record["per_prompt_elapsed_ms"][-1]
                    for record in measured
                    if record["mode"] == mode
                )
            ),
        }
        for mode in ("serial", "batch")
    }
    result = {
        "model_dir": str(model_dir),
        "model_type": config.get("model_type"),
        "quantization": config.get("quantization") or config.get("quantization_config"),
        "mlx_lm_version": version("mlx-lm"),
        "temperature": 0.1,
        "max_tokens": MAX_TOKENS,
        "batch_size": len(PROMPTS),
        "prompts": PROMPTS,
        "summary_excluding_first_repetition": summary,
        "runs": records,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"summary": summary, "output": str(args.output)}, indent=2))


if __name__ == "__main__":
    main()
