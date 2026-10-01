#!/usr/bin/env python3
"""Generate NNUE evaluation fixtures for the Rust engine parity test.

Reads distillgen validation data, runs the aetherl1rq (QAT) model on a sample
of positions, and writes "board:stm:rule:cp" lines where cp is exactly what
src/nnue.rs must produce (integer pipeline, same clamps and cp mapping).

Usage:
  python gen_nnue_fixtures.py -c <rundir>/run_config.yaml -p <ckpt> \
      -d <val .bin files...> -o tests/nnue_fixtures.txt [--num 3000]
"""

import argparse
import os
import random
import sys

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
TRAINER_DIR = os.path.join(HERE, "..", "..", "..", "rapfi", "Trainer")
sys.path.insert(0, TRAINER_DIR)

from dataset import build_dataset  # noqa: E402
from model import build_model  # noqa: E402
from model.aetherl1rq import SO  # noqa: E402
import yaml  # noqa: E402

CP_NUM, CP_SHIFT, CP_MAX = 9000, 1390, 10000


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("-c", "--config", required=True)
    parser.add_argument("-p", "--checkpoint", required=True)
    parser.add_argument("-d", "--data", nargs="+", required=True)
    parser.add_argument("-o", "--output", required=True)
    parser.add_argument("--num", type=int, default=3000)
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()

    cfg = yaml.safe_load(open(args.config))

    def _nested(v):
        return yaml.safe_load(v) if isinstance(v, str) else v

    model = build_model(cfg["model_type"], **(_nested(cfg.get("model_args", {})) or {}))
    state = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    model.load_state_dict(state["model"] if "model" in state else state, strict=False)
    model.eval()

    ds = build_dataset("distillgen_binary", args.data, shuffle=False, apply_symmetry=False)
    rng = random.Random(args.seed)
    idxs = sorted(rng.sample(range(len(ds)), min(args.num, len(ds))))

    lines = []
    with torch.no_grad():
        for i in idxs:
            item = ds[i]
            data = {
                k: (torch.as_tensor(v).unsqueeze(0)
                    if not torch.is_tensor(v) else v.unsqueeze(0))
                for k, v in item.items()
                if k in ("board_input", "stm_input", "rule_input")
            }
            value, _ = model(data)  # logit (float carrying score_q / SO)
            score_q = int(round(value[0, 0].item() * SO))
            a = abs(score_q)
            m = CP_NUM * a // (a + CP_SHIFT)
            cp = m if score_q >= 0 else -m
            cp = max(-CP_MAX, min(CP_MAX, cp))
            board = ds.boards[i]
            lines.append(
                f"{''.join(map(str, board.tolist()))}:{ds.stms[i]}:"
                f"{0 if ds.rule_inputs[i] < 0 else 1}:{cp}"
            )
    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w") as f:
        f.write("\n".join(lines) + "\n")
    print(f"wrote {len(lines)} fixtures to {args.output}")


if __name__ == "__main__":
    main()
