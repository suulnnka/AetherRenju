#!/usr/bin/env python3
"""Parity check: reload an exported l1r_f32.bin and recompute the forward
pass in pure numpy, comparing against the torch checkpoint on real positions
from distillgen validation data.

Usage:
    python check_f32_parity.py -b <weights/l1r_f32.bin> \
        -c <rundir>/run_config.yaml -p <checkpoint> [--data file.bin]
"""

import argparse
import os
import struct
import sys

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
TRAINER_DIR = os.path.join(HERE, "..", "..", "..", "rapfi", "Trainer")
sys.path.insert(0, TRAINER_DIR)

from dataset import build_dataset  # noqa: E402
from model import build_model  # noqa: E402
import yaml  # noqa: E402


def load_bin(path):
    blob = open(path, "rb").read()
    (crc,) = struct.unpack_from("<I", blob, len(blob) - 4)
    import zlib

    assert crc == (zlib.crc32(blob[:-4]) & 0xFFFFFFFF), "crc mismatch"
    magic, version, bsize, line_len, hidden, num_active = struct.unpack_from("<6I", blob, 0)
    off = 24
    active_ids = np.frombuffer(blob, np.uint16, num_active, off).astype(np.int64)
    off += 2 * num_active
    HW = bsize * bsize

    def take(count):
        nonlocal off
        arr = np.frombuffer(blob, "<f4", count, off).astype(np.float32).copy()
        off += 4 * count
        return arr

    e_hv = take(num_active * hidden).reshape(num_active, hidden)
    e_di = take(num_active * hidden).reshape(num_active, hidden)
    psq = take(2 * HW * hidden).reshape(2, HW, hidden)
    fc1w = take(hidden * (9 * hidden + 2)).reshape(hidden, 9 * hidden + 2)
    fc1b = take(hidden)
    fc2w = take(hidden).reshape(1, hidden)
    fc2b = take(1)
    tempo = take(1)
    assert off == len(blob) - 4, f"trailing bytes: {off} vs {len(blob)-4}"
    # id -> active row; dead ids (never hit by real boards) map to row -1
    row_of_id = np.full(3**line_len, -1, np.int64)
    row_of_id[active_ids] = np.arange(num_active)
    return dict(bsize=bsize, line_len=line_len, hidden=hidden, e_hv=e_hv, e_di=e_di,
                psq=psq, fc1w=fc1w, fc1b=fc1b, fc2w=fc2w, fc2b=fc2b, tempo=tempo[0],
                row_of_id=row_of_id, active_ids=active_ids)


def window_tables(bsize, line_len):
    half = line_len // 2
    dirs = [(1, 0), (0, 1), (1, 1), (1, -1)]
    HW = bsize * bsize
    idx = np.zeros((HW, 4, line_len), np.int64)
    valid = np.zeros((HW, 4), np.float32)
    for y in range(bsize):
        for x in range(bsize):
            for d, (dx, dy) in enumerate(dirs):
                ok = True
                for k in range(line_len):
                    off = k - half
                    xx, yy = x + dx * off, y + dy * off
                    if 0 <= xx < bsize and 0 <= yy < bsize:
                        idx[y * bsize + x, d, k] = yy * bsize + xx
                    else:
                        ok = False
                valid[y * bsize + x, d] = 1.0 if ok else 0.0
    powers = 3 ** np.arange(line_len, dtype=np.int64)
    return idx, valid, powers


def forward_np(w, idx, valid, powers, ternary, stm, rule):
    """ternary: [N, HW] int; stm/rule: [N] floats -> [N] stm-pov logit."""
    N, HW = ternary.shape
    hidden = w["hidden"]
    gathered = ternary[:, idx.reshape(-1)].reshape(N, HW, 4, idx.shape[2])
    ids = (gathered * powers).sum(-1)  # [N, HW, 4]
    rows = w["row_of_id"][ids]  # [N, HW, 4], -1 = dead (mixed) id
    row_idx = np.where(rows >= 0, rows, 0)  # placeholder row, killed by the mask
    # active mask over the full vocab: dead ids and off-board windows -> zero
    active = np.zeros(3 ** w["line_len"], np.float32)
    active[w["active_ids"]] = 1.0
    mask = valid[None, :, :] * active[ids]
    f = (
        w["e_hv"][row_idx[:, :, 0]] * mask[:, :, 0:1]
        + w["e_hv"][row_idx[:, :, 1]] * mask[:, :, 1:2]
        + w["e_di"][row_idx[:, :, 2]] * mask[:, :, 2:3]
        + w["e_di"][row_idx[:, :, 3]] * mask[:, :, 3:4]
    )
    f = f + np.where((ternary == 1)[..., None], w["psq"][0][None], np.where((ternary == 2)[..., None], w["psq"][1][None], 0.0))

    def screlu(x, c=16.0):
        return np.clip(x, 0.0, c) ** 2

    a = screlu(f)
    z = a.reshape(N, 3, 5, 3, 5, hidden).sum(axis=(2, 4)).reshape(N, 9 * hidden)
    head_in = np.concatenate([z, stm[:, None], rule[:, None]], axis=1)
    h = screlu(w["fc1w"] @ head_in.T + w["fc1b"][:, None]).T
    out_black = (h @ w["fc2w"].T)[:, 0] + w["fc2b"][0]
    sign = -stm
    return sign * (out_black + w["tempo"])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("-b", "--bin", required=True)
    parser.add_argument("-c", "--config", required=True)
    parser.add_argument("-p", "--checkpoint", required=True)
    parser.add_argument("--data", nargs="+")
    parser.add_argument("--num", type=int, default=20000)
    args = parser.parse_args()

    w = load_bin(args.bin)
    idx, valid, powers = window_tables(w["bsize"], w["line_len"])

    cfg = yaml.safe_load(open(args.config))

    def _nested(v):
        return yaml.safe_load(v) if isinstance(v, str) else v

    model_args = _nested(cfg.get("model_args", {})) or {}
    model = build_model(cfg["model_type"], **model_args)
    state = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    model.load_state_dict(state["model"] if "model" in state else state)
    model.eval()

    data_files = args.data or cfg["val_datas"]
    dataset = build_dataset(cfg["dataset_type"], data_files, shuffle=False,
                            **(_nested(cfg.get("dataset_args", {})) or {}))
    dataset.apply_symmetry = False  # identical inputs for both paths
    loader = torch.utils.data.DataLoader(dataset, batch_size=2048, num_workers=0)

    n, max_diff = 0, 0.0
    with torch.no_grad():
        for data in loader:
            value_t, _ = model(data)  # [B,1] stm-pov logit
            ternary = (data["board_input"][:, 0].long() + 2 * data["board_input"][:, 1].long()).flatten(1).numpy()
            value_n = forward_np(w, idx, valid, powers, ternary,
                                 data["stm_input"][:, 0].numpy(),
                                 data["rule_input"][:, 0].numpy())
            diff = np.abs(value_t[:, 0].numpy() - value_n)
            max_diff = max(max_diff, float(diff.max()))
            n += len(diff)
            if n >= args.num:
                break
    print(f"checked {n} positions: max |torch - numpy(bin)| = {max_diff:.2e}")
    assert max_diff < 1e-4, "parity check FAILED"
    print("parity OK")


if __name__ == "__main__":
    main()
