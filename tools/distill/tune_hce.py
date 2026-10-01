#!/usr/bin/env python3
"""Texel-style tuning of the HCE EVALS/EVALS_THREAT weights.

Model (exactly the engine's evaluation, linear in the weights):
    vb_p = Σ_pcode  cntb·E_slotB[pb] − cntw·E_slotW[pw]     (ply p ∈ {cur, prev})
    v    = (vb_cur + vb_prev)/2 + T_stm[mask]
    wr   = sigmoid(v / 250);   target wr_t = (wlr_teacher + 1) / 2
Loss  = MSE(wr, wr_t) + λ·[mean((E−E0)²) + mean((T−T0)²)]; weights clamped ±1400.

Table dispatch: freestyle -> E_F both colors, T_F; renju -> E_RB/E_RW,
T_RB/T_RW (by stm). Data = dump_features 输出(两拍稀疏直方图 + 掩码)。

Usage:
  python tune_hce.py --train /tmp/feat_train.bin --val /tmp/feat_val.bin \
      --model .../model220723.bin --out-tuned-model /tmp/model_tuned.bin \
      --out-tables src/eval_tables.bin
"""

import argparse
import struct

import lz4.frame
import numpy as np
import torch

N_E, N_T, SCALE = 3876, 2048, 250.0
CLAMP = 1400.0


def load_records(path):
    """path 为前缀:读取 <path>.rec / <path>.ent(dump_features 输出)。"""
    rec = np.fromfile(path + '.rec', dtype=np.uint8)
    n = int(np.frombuffer(rec[:4], '<u4')[0])
    rec = rec[4:].reshape(n, 10)
    ent = np.fromfile(path + '.ent', dtype=np.uint8)
    m = int(np.frombuffer(ent[:4], '<u4')[0])
    ent = ent[4:].reshape(m, 8)
    erow = np.frombuffer(ent[:, :4].copy(), '<u4').astype(np.int64)
    efidx = np.frombuffer(ent[:, 4:6].copy(), '<u2').astype(np.int64)
    ecoef = (np.frombuffer(ent[:, 6:8].copy(), '<i2').astype(np.float32)) * 0.5
    return dict(
        n=n,
        row=erow,
        fidx=efidx,
        coef=ecoef,
        rule=rec[:, 0].copy(),
        stm=rec[:, 1].copy(),
        label=np.frombuffer(rec[:, 4:8].copy(), '<f4').copy(),
        mask=np.frombuffer(rec[:, 8:10].copy(), '<u2').astype(np.int64),
    )


def to_device(D, device):
    return {k: (torch.from_numpy(v).to(device) if isinstance(v, np.ndarray) else v)
            for k, v in D.items()}


def eval_mse(W, T, D, bs=65536, q=False):
    Wd, Td = W.detach(), T.detach()
    if q:
        Wd, Td = torch.round(Wd), torch.round(Td)
    tot, cnt = 0.0, 0
    n = D['n']
    row = D['row']
    for s in range(0, n, bs):
        e = min(s + bs, n)
        lo = int(torch.searchsorted(row, torch.tensor(s, device=row.device)))
        hi = int(torch.searchsorted(row, torch.tensor(e, device=row.device)))
        v = torch.zeros(e - s, device=Wd.device)
        v.index_add_(0, row[lo:hi] - s, Wd[D['fidx'][lo:hi]] * D['coef'][lo:hi])
        tidx = torch.where(D['rule'][s:e] == 0,
                           torch.zeros_like(D['mask'][s:e]) * 0,
                           torch.where(D['stm'][s:e] == 0,
                                       torch.ones_like(D['mask'][s:e]),
                                       2 * torch.ones_like(D['mask'][s:e])))
        v += Td[tidx * N_T + D['mask'][s:e]]
        wr = torch.sigmoid(v / SCALE)
        t = (D['label'][s:e] + 1) / 2
        tot += ((wr - t) ** 2).sum().item()
        cnt += e - s
    return tot / cnt


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--train', required=True)
    ap.add_argument('--val', required=True)
    ap.add_argument('--model', required=True)
    ap.add_argument('--out-tuned-model')
    ap.add_argument('--out-tables')
    ap.add_argument('--epochs', type=int, default=4)
    ap.add_argument('--bs', type=int, default=16384)
    ap.add_argument('--lr', type=float, default=0.3)
    ap.add_argument('--anchor', type=float, default=1e-5)
    ap.add_argument('--freeze-threat', action='store_true',
                    help='威胁表全冻结(教师概率标签会把决胜项压软,只调 EVALS)')
    ap.add_argument('--subsample', type=int, default=500000)
    ap.add_argument('--seed', type=int, default=0,
                    help=' 固定随机种子(子采样 + 批序),结果可复现;0 = 不固定')
    args = ap.parse_args()

    if args.seed:
        torch.manual_seed(args.seed)
        np.random.seed(args.seed)

    device = 'cuda' if torch.cuda.is_available() else 'cpu'
    print('device:', device)

    raw = lz4.frame.decompress(open(args.model, 'rb').read())
    off = 8
    EVALS = np.frombuffer(raw, '<i2', 4 * N_E, off).reshape(4, N_E).astype(np.float32).copy()
    off += 4 * N_E * 2
    THREAT = np.frombuffer(raw, '<i2', 4 * N_T, off).reshape(4, N_T).astype(np.float32).copy()
    off += 4 * N_T * 2
    P4 = np.frombuffer(raw, '<i2', 4 * N_E * 2, off).reshape(4, N_E, 2).copy()

    W0 = torch.tensor(np.concatenate([EVALS[0], EVALS[2], EVALS[3]]), device=device)
    T0 = torch.tensor(np.concatenate([THREAT[0], THREAT[2], THREAT[3]]), device=device)

    print('loading features ...')
    Dtr = to_device(load_records(args.train), device)
    Dv = to_device(load_records(args.val), device)
    if Dtr['n'] > args.subsample:
        keep = torch.randperm(Dtr['n'], device=device)[: args.subsample]
        keep = torch.sort(keep).values
        Dtr = filter_by_rows(Dtr, keep)
    print(f"train {Dtr['n']}  val {Dv['n']}  entries {len(Dtr['fidx'])}")

    base = eval_mse(W0, T0, Dv)
    print(f'baseline val winrate-MSE: {base:.5f}')

    W = W0.clone().requires_grad_(True)
    T = T0.clone().requires_grad_(not args.freeze_threat)
    opt = torch.optim.Adam([W, T], lr=args.lr)

    n = Dtr['n']
    row = Dtr['row']
    nb = (n + args.bs - 1) // args.bs
    for ep in range(args.epochs):
        order = torch.randperm(nb)
        for bi in order:
            s, e = int(bi) * args.bs, min(int(bi) * args.bs + args.bs, n)
            lo = int(torch.searchsorted(row, torch.tensor(s, device=row.device)))
            hi = int(torch.searchsorted(row, torch.tensor(e, device=row.device)))
            v = torch.zeros(e - s, device=device)
            v.index_add_(0, row[lo:hi] - s, W[Dtr['fidx'][lo:hi]] * Dtr['coef'][lo:hi])
            tidx = torch.where(Dtr['rule'][s:e] == 0,
                               torch.zeros_like(Dtr['mask'][s:e]),
                               torch.where(Dtr['stm'][s:e] == 0,
                                           torch.ones_like(Dtr['mask'][s:e]),
                                           2 * torch.ones_like(Dtr['mask'][s:e])))
            v += T[tidx * N_T + Dtr['mask'][s:e]]
            wr = torch.sigmoid(v / SCALE)
            t = (Dtr['label'][s:e] + 1) / 2
            loss = ((wr - t) ** 2).mean()
            loss = loss + args.anchor * (((W - W0) ** 2).mean() + ((T - T0) ** 2).mean())
            opt.zero_grad()
            loss.backward()
            opt.step()
            with torch.no_grad():
                W.clamp_(-CLAMP, CLAMP)
                T.clamp_(-CLAMP, CLAMP)
                # 掩码 0(无任何威胁)钉在原值:保留 rapfi 的先手常数,
                # 防止调参把它变成破坏零和的大偏置
                T[0] = T0[0]
                T[N_T] = T0[N_T]
                T[2 * N_T] = T0[2 * N_T]
        vmse = eval_mse(W, T, Dv)
        print(f'epoch {ep + 1}: val winrate-MSE {vmse:.5f} (baseline {base:.5f})')

    qmse = eval_mse(W, T, Dv, q=True)
    print(f'quantized(i16-rounded) val MSE: {qmse:.5f}')

    with torch.no_grad():
        Wq = torch.round(W).clamp(-1400, 1400).to(torch.int16).cpu().numpy()
        Tq = torch.round(T).clamp(-1400, 1400).to(torch.int16).cpu().numpy()
    E_F, E_RB, E_RW = Wq[:N_E], Wq[N_E:2 * N_E], Wq[2 * N_E:]
    T_F, T_RB, T_RW = Tq[:N_T], Tq[N_T:2 * N_T], Tq[2 * N_T:]

    if args.out_tables:
        blob = struct.pack('<4sIf', b'AHCE', 1, SCALE)
        for a in (E_F, E_RB, E_RW, T_F, T_RB, T_RW):
            blob += a.astype('<i2').tobytes()
        open(args.out_tables, 'wb').write(blob)
        print(f'wrote {args.out_tables} ({len(blob)} bytes)')

    if args.out_tuned_model:
        evals = np.stack([E_F, EVALS[1].astype(np.int16), E_RB, E_RW])  # standard 槽保持原值
        threat = np.stack([T_F, THREAT[1].astype(np.int16), T_RB, T_RW])
        out = struct.pack('<d', SCALE)
        out += evals.astype('<i2').tobytes()
        out += threat.astype('<i2').tobytes()
        out += P4.astype('<i2').tobytes()
        open(args.out_tuned_model, 'wb').write(lz4.frame.compress(out))
        print(f'wrote {args.out_tuned_model} ({len(out)} raw)')


def filter_by_rows(D, keep):
    """按样本行过滤摊平数组(keep 需升序)。"""
    row = D['row']
    device = row.device
    n = D['n']
    lo = torch.searchsorted(row, keep, right=False)
    hi = torch.searchsorted(row, keep, right=True)
    counts = (hi - lo).tolist()
    starts = lo.tolist()
    sel = torch.cat([torch.arange(s, s + c, device=device)
                     for s, c in zip(starts, counts)]) if counts else torch.zeros(0, dtype=torch.long, device=device)
    remap = torch.zeros(n, dtype=torch.long, device=device)
    remap[keep] = torch.arange(len(keep), device=device)
    return dict(
        n=len(keep),
        row=remap[D['row'][sel]],
        fidx=D['fidx'][sel],
        coef=D['coef'][sel],
        rule=D['rule'][keep.cpu().numpy()],
        stm=D['stm'][keep.cpu().numpy()],
        label=D['label'][keep.cpu().numpy()],
        mask=D['mask'][keep.cpu().numpy()],
    )


if __name__ == '__main__':
    main()
