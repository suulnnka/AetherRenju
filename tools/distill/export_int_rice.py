#!/usr/bin/env python3
"""Export a trained aetherl1rq (QAT) checkpoint to a Rice-coded int8/int16
net blob for the AetherRenju engine (src/nnue.rs decodes it at init).

Layout "ALR2" (all little endian; zigzag + MSB-first Golomb-Rice streams,
per-256-value adaptive k for the i8 sections, global k for i16 sections):

  u32 magic 0x32524C41 ("ALR2")   u32 version 2
  u32 boardSize=15  u32 lineLen=7  u32 hidden=16  u32 numActive=919
  u16 activeIds[919]              id -> row mapping (ascending)
  u32 QE CF QW1 S1 SH CH QW2 S2 SO CP_NUM CP_SHIFT CP_MAX   (deploy constants)
  u32 nBlk  u8 k_ehv[nBlk] u8 k_edi[nBlk] u8 k_psq[nBlk]
  u8 k_fc1w k_fc1b k_fc2w k_fc2bt
  u32 len[7]                      bitstream byte lengths
  <7 rice bitstreams, byte-aligned back to back>
     e_hv[919*16]i8  e_di[919*16]i8  psq[2*225*16]i8
     fc1w[16*146]i16 fc1b[16]i16 fc2w[16]i16 fc2b_tempo[2]i16
  u32 crc32                       over everything above

Usage:
  python export_int_rice.py -c <rundir>/run_config.yaml -p <ckpt> \
      -o ../../src/l1r.nnue.rice [--meta-out ...]
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

MAGIC = 0x32524C41  # "ALR2"
VERSION = 2
BLK = 256

from model.aetherl1rq import QE, CF, QW1, S1, SH, CH, QW2, S2, SO  # noqa: E402

CP_NUM, CP_SHIFT, CP_MAX = 9000, 1390, 10000  # 饱和映射 cp = CP_NUM*a/(a+CP_SHIFT)


def zz(v):
    return (v << 1) ^ (v >> 31) if v >= 0 else -v * 2 - 1


class BW:
    def __init__(self):
        self.out = bytearray()
        self.acc = 0
        self.nb = 0

    def wbit(self, b):
        self.acc = (self.acc << 1) | b
        self.nb += 1
        if self.nb == 8:
            self.out.append(self.acc)
            self.acc = 0
            self.nb = 0

    def rice(self, v, k):
        u = zz(v)
        q = u >> k
        for _ in range(q):
            self.wbit(0)
        self.wbit(1)
        for i in range(k - 1, -1, -1):
            self.wbit((u >> i) & 1)

    def done(self):
        if self.nb:
            self.out.append((self.acc << (8 - self.nb)) & 0xFF)
            self.acc = 0
            self.nb = 0
        return bytes(self.out)


class BR:
    def __init__(self, s):
        self.s = s
        self.p = 0
        self.acc = 0
        self.nb = 0

    def bit(self):
        if self.nb == 0:
            self.acc = self.s[self.p]
            self.p += 1
            self.nb = 8
        self.nb -= 1
        return (self.acc >> self.nb) & 1

    def rice(self, k):
        q = 0
        while self.bit() == 0:
            q += 1
        r = 0
        for _ in range(k):
            r = (r << 1) | self.bit()
        u = (q << k) | r
        return (u >> 1) ^ -(u & 1)


def bits_of(vals, k):
    return sum((zz(v) >> k) + 1 + k for v in vals)


def encode_i8_sections(sections):
    """sections: list of int arrays; each gets per-256-block adaptive k,
    encoded as ONE contiguous bitstream per section."""
    streams, kss, nblk = [], [], (max(len(s) for s in sections) + BLK - 1) // BLK
    for vals in sections:
        w = BW()
        ks = []
        for b in range((len(vals) + BLK - 1) // BLK):
            chunk = vals[b * BLK : (b + 1) * BLK]
            k = min(range(17), key=lambda k: bits_of(chunk, k))
            ks.append(k)
            for v in chunk:
                w.rice(v, k)
        ks += [0] * (nblk - len(ks))  # pad k table for short sections
        streams.append(w.done())
        kss.append(ks)
    return streams, kss, nblk


def encode_i16_sections(sections):
    streams, ks = [], []
    for vals in sections:
        k = min(range(17), key=lambda k: bits_of(vals, k))
        w = BW()
        for v in vals:
            w.rice(v, k)
        streams.append(w.done())
        ks.append(k)
    return streams, ks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("-c", "--config", required=True)
    parser.add_argument("-p", "--checkpoint", required=True)
    parser.add_argument("-o", "--output", required=True)
    parser.add_argument("--meta-out", default=None)
    parser.add_argument("--note", default="")
    args = parser.parse_args()

    cfg = yaml.safe_load(open(args.config))

    def _nested(v):
        return yaml.safe_load(v) if isinstance(v, str) else v

    model_args = _nested(cfg.get("model_args", {})) or {}
    model = build_model(cfg["model_type"], **model_args)
    state = torch.load(args.checkpoint, map_location="cpu", weights_only=False)
    model.load_state_dict(state["model"] if "model" in state else state, strict=False)
    model.eval()
    assert type(model).__name__ == "AetherL1RQNet", "export_int_rice needs an aetherl1rq (QAT) checkpoint"

    sd = model.state_dict()

    # active ids in ascending order (engine builds id->row at init; export maps rows)
    active_ids = model._active_row_mask(model.vocab, model.line_len).nonzero().flatten().tolist()
    assert len(active_ids) == 919

    # integer weights (exactly what the QAT forward used)
    e_hv = torch.round(sd["e_hv"] * QE).clamp(-127, 127).numpy().astype(np.int64)[active_ids]
    e_di = torch.round(sd["e_di"] * QE).clamp(-127, 127).numpy().astype(np.int64)[active_ids]
    psq = torch.round(sd["psq"] * QE).clamp(-127, 127).numpy().astype(np.int64)
    fc1w = torch.round(sd["head_fc1.weight"] * QW1).clamp(-32768, 32767).numpy().astype(np.int64)
    fc1b = torch.round(sd["head_fc1.bias"] * SH).clamp(-32768, 32767).numpy().astype(np.int64)
    fc2w = torch.round(sd["head_fc2.weight"] * QW2).clamp(-32768, 32767).numpy()[0].astype(np.int64)
    fc2b = torch.round(sd["head_fc2.bias"] * SO).clamp(-32768, 32767).numpy().astype(np.int64)
    tempo = torch.round(sd["tempo"] * SO).clamp(-32768, 32767).numpy().astype(np.int64)

    i8_secs = [e_hv.reshape(-1).tolist(), e_di.reshape(-1).tolist(), psq.reshape(-1).tolist()]
    i16_secs = [
        fc1w.reshape(-1).tolist(),
        fc1b.tolist(),
        fc2w.tolist(),
        fc2b.tolist() + tempo.tolist(),
    ]
    st8, ks8, nblk = encode_i8_sections(i8_secs)
    st16, ks16 = encode_i16_sections(i16_secs)

    blob = struct.pack(
        "<2I4I", MAGIC, VERSION, model.board_size, model.line_len, model.hidden, len(active_ids)
    )
    blob += np.asarray(active_ids, dtype=np.uint16).tobytes()
    blob += struct.pack(
        "<11I", QE, CF, QW1, S1, SH, CH, QW2, S2, SO, CP_NUM, CP_SHIFT
    )
    blob += struct.pack("<I", nblk)
    blob += bytes(ks8[0]) + bytes(ks8[1]) + bytes(ks8[2])
    blob += bytes([ks16[0], ks16[1], ks16[2], ks16[3]])
    lens = [len(s) for s in st8 + st16]
    blob += struct.pack("<7I", *lens)
    for s in st8 + st16:
        blob += s
    blob += struct.pack("<I", zlib.crc32(blob) & 0xFFFFFFFF)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    open(args.output, "wb").write(blob)

    # ---- roundtrip verification (mirror of the Rust decoder) ----
    p = 0
    assert struct.unpack_from("<I", blob, p)[0] == MAGIC
    p = 8
    bsize, llen, hidden, nact = struct.unpack_from("<4I", blob, p)
    p += 16
    aids = np.frombuffer(blob, np.uint16, nact, p).tolist()
    p += 2 * nact
    consts = struct.unpack_from("<11I", blob, p)
    p += 44
    (nblk2,) = struct.unpack_from("<I", blob, p)
    p += 4
    k8 = []
    for _ in range(3):
        k8.append(blob[p : p + nblk2])
        p += nblk2
    k16 = blob[p : p + 4]
    p += 4
    lens2 = struct.unpack_from("<7I", blob, p)
    p += 28
    assert (bsize, llen, hidden, nact) == (15, 7, 16, 919)
    assert (nblk2, lens2) == (nblk, tuple(lens))
    got = []
    base = p
    for si in range(3):
        r = BR(blob[base : base + lens[si]])
        vals = []
        idx = 0
        tot = len(i8_secs[si])
        for b in range((tot + BLK - 1) // BLK):
            for _ in range(min(BLK, tot - b * BLK)):
                vals.append(r.rice(k8[si][b]))
        got.append(vals)
        base += lens[si]
    for si in range(4):
        r = BR(blob[base : base + lens[3 + si]])
        got.append([r.rice(k16[si]) for _ in i16_secs[si]])
        base += lens[3 + si]
    for a, b in zip(got, i8_secs + i16_secs):
        assert a == b, "rice roundtrip failed"
    assert base == len(blob) - 4
    print(f"exported {args.output}: {len(blob)} bytes (rice)")
    print(f"  sections: e_hv={lens[0]}B e_di={lens[1]}B psq={lens[2]}B "
          f"fc1w={lens[3]}B fc1b={lens[4]}B fc2w={lens[5]}B fc2bt={lens[6]}B")
    import gzip

    gz = len(gzip.compress(blob, 9))
    print(f"  gzip on top: {gz} bytes (weights contribution to wasm budget)")

    if args.meta_out:
        meta = {
            "file": os.path.basename(args.output),
            "size_bytes": len(blob),
            "gzip_bytes": gz,
            "format": {"magic": hex(MAGIC), "version": VERSION},
            "constants": dict(zip(
                ["QE", "CF", "QW1", "S1", "SH", "CH", "QW2", "S2", "SO", "CP_NUM", "CP_SHIFT"],
                consts,
            )),
            "checkpoint": os.path.abspath(args.checkpoint),
            "note": args.note,
        }
        with open(args.meta_out, "w") as f:
            json.dump(meta, f, indent=2, ensure_ascii=False)
        print(f"metadata written to {args.meta_out}")


if __name__ == "__main__":
    main()
