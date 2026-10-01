#!/usr/bin/env python3
"""Evaluate a trained aetherl1r checkpoint on distillgen validation data.

Usage (from rapfi/Trainer):
    python ../../AetherRenju/tools/distill/eval_l1r.py \
        -c <rundir>/run_config.yaml -p <rundir>/ckpt_aetherl1r_h16_xxxxxxx \
        [--data file1.bin file2.bin ...]

Reports winrate MSE, WLR MAE, sign agreement and ply/rule breakdowns
against the mix9svq teacher labels.
"""

import argparse
import json
import os
import sys

import numpy as np
import torch

TRAINER_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "rapfi", "Trainer")
sys.path.insert(0, TRAINER_DIR)

from dataset import build_dataset  # noqa: E402
from model import build_model  # noqa: E402
import yaml  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("-c", "--config", required=True, help="run_config.yaml of the training run")
    parser.add_argument("-p", "--checkpoint", required=True, help="checkpoint file")
    parser.add_argument("--data", nargs="+", help="override validation data files")
    parser.add_argument("--batch_size", type=int, default=4096)
    args = parser.parse_args()

    cfg = yaml.safe_load(open(args.config))

    def _nested(v):
        return yaml.safe_load(v) if isinstance(v, str) else v

    model_args = _nested(cfg.get("model_args", {})) or {}
    dataset_args = _nested(cfg.get("dataset_args", {})) or {}
    model = build_model(cfg["model_type"], **model_args)
    state = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    state_dict = state["model"] if "model" in state else state
    model.load_state_dict(state_dict)
    model.eval()

    data_files = args.data or cfg["val_datas"]
    dataset = build_dataset(cfg["dataset_type"], data_files, shuffle=False, **dataset_args)
    loader = torch.utils.data.DataLoader(dataset, batch_size=args.batch_size, num_workers=2)

    all_pred, all_tgt, all_rule, all_ply = [], [], [], []
    with torch.no_grad():
        for data in loader:
            value, _ = model(data)
            pred_wlr = 2 * torch.sigmoid(value[:, 0]) - 1  # stm-pov wlr
            tgt = data["value_target"]
            tgt_wlr = tgt[:, 0] - tgt[:, 1]
            all_pred.append(pred_wlr.numpy())
            all_tgt.append(tgt_wlr.numpy())
            all_rule.append(data["rule_input"][:, 0].numpy())
            all_ply.append(np.asarray(data["ply"]))

    pred = np.concatenate(all_pred)
    tgt = np.concatenate(all_tgt)
    rule = np.concatenate(all_rule)
    ply = np.concatenate(all_ply)

    def report(mask, name):
        if mask.sum() == 0:
            return
        p, t = pred[mask], tgt[mask]
        mse_wr = np.mean(((p + 1) / 2 - (t + 1) / 2) ** 2)
        mae = np.mean(np.abs(p - t))
        agree = np.mean(np.sign(p) == np.sign(t))
        big = np.mean(np.abs(p - t) > 0.5)
        print(
            f"  {name:<28} n={mask.sum():>8}  winrateMSE={mse_wr:.4f}  wlrMAE={mae:.4f}  "
            f"signAgree={agree:.2%}  |err|>0.5={big:.2%}"
        )
        return mse_wr, mae, agree

    print(f"checkpoint: {args.checkpoint}")
    print(f"entries:    {len(pred)}")
    report(np.ones(len(pred), bool), "ALL")
    report(rule < 0, "freestyle (no forbidden)")
    report(rule > 0, "renju (forbidden)")
    for lo, hi in [(0, 5), (5, 15), (15, 30), (30, 60), (60, 999)]:
        report((ply >= lo) & (ply < hi), f"ply [{lo},{hi})")
    # calibration buckets
    print("  calibration (mean pred vs mean target by target bucket):")
    for lo in np.arange(-1.0, 1.0, 0.25):
        m = (tgt >= lo) & (tgt < lo + 0.25)
        if m.sum() > 100:
            print(f"    teacher wlr [{lo:+.2f},{lo+0.25:+.2f}): n={m.sum():>7}  pred={pred[m].mean():+.3f}")


if __name__ == "__main__":
    main()
