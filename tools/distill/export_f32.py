#!/usr/bin/env python3
"""Export a trained aetherl1r checkpoint to a flat f32 weight file.

Quantization and engine integration are deliberately out of scope here; this
dumps f32 weights in the layout the future wasm loader will use, with the
919 active vocabulary rows of the embedding tables (dead mixed-color rows are
never hit by a real position and get zero gradients, so dropping them is
lossless - asserted against the checkpoint).

Binary layout (all little endian, f32 blocks contiguous):

    u32 magic = 0x31524C41 ('ALR1')
    u32 version = 1
    u32 boardSize = 15
    u32 lineLen = 7
    u32 hidden = 16
    u32 numActive = 919
    u16 activeIds[numActive]        id -> row mapping (sorted)
    f32 eHv[numActive * hidden]     row-major
    f32 eDi[numActive * hidden]
    f32 psq[2 * 225 * hidden]       [color][cell][h]
    f32 headFc1Weight[hidden][9*hidden + 2]
    f32 headFc1Bias[hidden]
    f32 headFc2Weight[1][hidden]
    f32 headFc2Bias[1]
    f32 tempo[1]
    u32 crc32                       over everything above

Usage:
    python export_f32.py -c <rundir>/run_config.yaml -p <checkpoint> \
        -o ../../weights/l1r_f32.bin [--meta-out ../../weights/l1r_f32.json]
"""

import argparse
import json
import os
import struct
import sys
import zlib

import numpy as np
import torch

TRAINER_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "rapfi", "Trainer")
sys.path.insert(0, TRAINER_DIR)

from model import build_model  # noqa: E402
import yaml  # noqa: E402

MAGIC = 0x31524C41
VERSION = 1


def active_pattern_ids(line_len: int) -> list[int]:
    """Ids with at least one 5-slot sub-window that uses a single color at
    most (all empty, or empty + one color). 919 for line_len=7 (matches
    nnue-design.md §3.1)."""
    active = []
    for vid in range(3**line_len):
        cells = []
        v = vid
        for _ in range(line_len):
            cells.append(v % 3)
            v //= 3
        for start in range(line_len - 4):
            colors = set(cells[start : start + 5]) - {0}
            if len(colors) <= 1:
                active.append(vid)
                break
    return active


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("-c", "--config", required=True)
    parser.add_argument("-p", "--checkpoint", required=True)
    parser.add_argument("-o", "--output", required=True)
    parser.add_argument("--meta-out", default=None, help="json metadata path")
    parser.add_argument("--note", default="", help="free-form note stored in metadata")
    args = parser.parse_args()

    cfg = yaml.safe_load(open(args.config))

    def _nested(v):
        return yaml.safe_load(v) if isinstance(v, str) else v

    model_args = _nested(cfg.get("model_args", {})) or {}
    model = build_model(cfg["model_type"], **model_args)
    state = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    model.load_state_dict(state["model"] if "model" in state else state)
    model.eval()

    board_size = model.board_size
    line_len = model.line_len
    hidden = model.hidden

    active_ids = active_pattern_ids(line_len)
    num_active = len(active_ids)
    assert num_active == 919, f"expected 919 active ids for L=7, got {num_active}"

    sd = model.state_dict()
    e_hv, e_di, psq = sd["e_hv"].numpy(), sd["e_di"].numpy(), sd["psq"].numpy()
    fc1w, fc1b = sd["head_fc1.weight"].numpy(), sd["head_fc1.bias"].numpy()
    fc2w, fc2b = sd["head_fc2.weight"].numpy(), sd["head_fc2.bias"].numpy()
    tempo = sd["tempo"].numpy()

    # dead rows are masked to zero in forward, so they never receive gradient;
    # they may drift slightly below init from AdamW weight decay. They are
    # dropped at export, which is lossless by construction.
    dead_mask = np.ones(len(e_hv), dtype=bool)
    dead_mask[active_ids] = False
    dead_absmax = float(np.abs(e_hv[dead_mask]).max())
    dead_absmax_di = float(np.abs(e_di[dead_mask]).max())
    print(f"dead-row abs max (masked in forward, dropped at export; init was 0.05): "
          f"e_hv={dead_absmax:.4f} e_di={dead_absmax_di:.4f}")

    header = struct.pack(
        "<6I", MAGIC, VERSION, board_size, line_len, hidden, num_active
    ) + np.asarray(active_ids, dtype=np.uint16).tobytes()

    body = b"".join(
        arr.astype("<f4").tobytes()
        for arr in (
            e_hv[active_ids],
            e_di[active_ids],
            psq,
            fc1w,
            fc1b,
            fc2w,
            fc2b,
            tempo,
        )
    )
    blob = header + body + struct.pack("<I", zlib.crc32(header + body) & 0xFFFFFFFF)
    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "wb") as f:
        f.write(blob)

    n_params = (
        2 * num_active * hidden + 2 * board_size * board_size * hidden
        + fc1w.size + fc1b.size + fc2w.size + fc2b.size + tempo.size
    )
    print(f"exported {args.output}: {len(blob)} bytes, {n_params} active f32 params")

    if args.meta_out:
        meta = {
            "file": os.path.basename(args.output),
            "size_bytes": len(blob),
            "crc32": hex(zlib.crc32(header + body) & 0xFFFFFFFF),
            "format": {"magic": hex(MAGIC), "version": VERSION},
            "shape": {
                "board_size": board_size,
                "line_len": line_len,
                "vocab": 3**line_len,
                "active_vocab": num_active,
                "hidden": hidden,
                "screlu_clamp": model.screlu_clamp,
            },
            "active_params": int(n_params),
            "checkpoint": os.path.abspath(args.checkpoint),
            "config": os.path.abspath(args.config),
            "note": args.note,
        }
        with open(args.meta_out, "w") as f:
            json.dump(meta, f, indent=2, ensure_ascii=False)
        print(f"metadata written to {args.meta_out}")


if __name__ == "__main__":
    main()
