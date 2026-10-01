#!/usr/bin/env python3
"""Generate HCE parity fixtures: reconstruct move sequences from distillgen
validation games, evaluate them with rapfi's classical evaluator (evaldump),
and write "rule m1,m2,...:value" lines for the Rust test."""
import sys, subprocess, struct
sys.path.insert(0, '/home/a/renju/rapfi/Trainer')
from dataset.distillgen import load_distillgen_file
import numpy as np

Rapfi = '/home/a/renju/rapfi/Rapfi/build/distill/pbrain-rapfi'
Model = sys.argv[1] if len(sys.argv) > 1 else '/home/a/renju/rapfi/Networks/classical/model220723.bin'

def moves_of_game(boards):
    """consecutive board ternaries -> move cell list(boards[0] 为空盘)"""
    moves = []
    prev = boards[0].astype(np.int16)
    for b in boards[1:]:
        diff = np.nonzero(b.astype(np.int16) - prev)[0]
        assert len(diff) == 1, f"diff {len(diff)}"
        moves.append(int(diff[0]))
        prev = b.astype(np.int16)
    return moves

out_lines = []
rng = np.random.default_rng(11)
for path, rule_code in [
    ('/home/a/renju/AetherRenju/tools/distill/data/fs_nat_val.bin', 0),
    ('/home/a/renju/AetherRenju/tools/distill/data/rj_nat_val.bin', 1),
]:
    # reconstruct per-game board sequences from the flat dump
    data = open(path, 'rb').read()
    off = 26
    games = []
    for g in range(struct.unpack_from('<I', data, 14)[0]):
        npos, = struct.unpack_from('<H', data, off); off += 3
        boards = []
        for i in range(npos):
            boards.append(np.frombuffer(data, np.uint8, 225, off + 2).copy()); off += 718
        games.append(boards)
    # sample up to 12 positions per game (plies >=1), max ~800 lines/rule
    count = 0
    for boards in games:
        moves = moves_of_game(boards)
        for ply in sorted(rng.choice(range(1, len(moves)), size=min(10, len(moves) - 1), replace=False)):
            if count >= 800:
                break
            ms = moves[: ply + 1]
            coords = " ".join(f"{m % 15},{m // 15}" for m in ms)
            out_lines.append(f"{rule_code} {coords}")
            count += 1
        if count >= 800:
            break

open('/tmp/hce_positions.txt', 'w').write("\n".join(out_lines) + "\n")
print(f"{len(out_lines)} positions -> /tmp/hce_positions.txt")
res = subprocess.run([Rapfi, 'evaldump', Model, '/tmp/hce_positions.txt'], capture_output=True, text=True)
vals = [int(x) for x in res.stdout.split()]
assert len(vals) == len(out_lines), (len(vals), len(out_lines))
with open('/home/a/renju/AetherRenju/tests/eval_fixtures.txt', 'w') as f:
    for line, v in zip(out_lines, vals):
        f.write(f"{line}:{v}\n")
print(f"wrote {len(vals)} fixtures (values range {min(vals)}..{max(vals)})")
