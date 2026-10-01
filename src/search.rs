/* ============================================================
 * 搜索:negamax + PVS 零窗口 + 期望窗口
 * + 迭代加深 + 双槽置换表 + 威胁分级排序 + killer/history/连续历史
 * + 威胁门控 LMR/LMP/IIR + 强迫步静态搜索 + 根 VCT(vct.rs)。
 * 节点预算为主(可复现)、墙上时间兜底;搜索零分配。
 *
 * 排序键打包(i32):tier<<24 | killer<<23 | 增益/history/连续历史
 * (bit 30 = TT 着法)。forcing = tier≥2:LMR/LMP 永不减免威胁着
 * (对强制着做朴素 LMR 代价惨重,强制着一律不减)。
 * ============================================================ */
use crate::*;

/* ==================== 预算与中止 ==================== */
pub struct Ctx {
    pub nodes: u64,
    pub node_budget: u64,
    pub ms_budget: u64,
    pub t0: f64,
    pub aborted: bool,
    pub qs_cap: usize,
    pub mode: i32,
    /* 结果(search_best 填写,api 层读取) */
    pub best: i32,
    pub best_score: i32,
    pub done_depth: i32,
    pub mate: bool,
    pub only: bool,
    pub draw: bool,
    pub root_len: usize,
    pub root_moves: [i32; SIZE + 1],
    pub root_scores: [i32; SIZE + 1],
}

pub static mut S: Ctx = Ctx {
    nodes: 0,
    node_budget: 0,
    ms_budget: 0,
    t0: 0.0,
    aborted: false,
    qs_cap: 14,
    mode: RENJU,
    best: -1,
    best_score: 0,
    done_depth: 0,
    mate: false,
    only: false,
    draw: false,
    root_len: 0,
    root_moves: [0; SIZE + 1],
    root_scores: [0; SIZE + 1],
};

#[inline]
pub unsafe fn s() -> *mut Ctx {
    core::ptr::addr_of_mut!(S)
}

/* 墙上时间:wasm 从 JS 导入 performance.now();原生用 std Instant
 * (crate 仅在 wasm32 上 no_std)。原生此前无时钟、时间兜底不触发,
 * 全靠节点预算;现在原生同样受毫秒预算约束。 */
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn now() -> f64;
}
#[cfg(target_arch = "wasm32")]
unsafe fn now_ms() -> f64 {
    unsafe { now() }
}
#[cfg(not(target_arch = "wasm32"))]
unsafe fn now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

#[inline]
pub unsafe fn time_up() -> bool {
    (*s()).nodes += 0; // 保持与旧版「每节点先计数」的语义一致性占位
    if (*s()).nodes > (*s()).node_budget {
        (*s()).aborted = true;
        return true;
    }
    if (*s()).nodes & 1023 == 0 && now_ms() - (*s()).t0 > (*s()).ms_budget as f64 {
        (*s()).aborted = true;
        return true;
    }
    false
}

/* 置换表(双槽桶):depth-preferred + always-replace,
 * 新深条目挤入 depth 槽并把异键旧条目压入 always 槽。
 * 每次 search_best 前整表清零(可复现优先)。 */

const TT_BUCKETS: usize = 1 << 16; // 64K 桶 × 2 槽 × 16B = 2 MB
const TT_MASK: u64 = (TT_BUCKETS - 1) as u64;

#[derive(Clone, Copy)]
struct TtSlot {
    key: u64,
    sc: i32,
    mv: i32,
    dp: i8,
    fl: u8, // 0 exact / 2 lower / 3 upper(与旧版编码一致)
}
/* 空槽 = 全零(key == 0):静态区零初始化不占 wasm 文件体积。
 * 空盘 Zobrist 恰为 0,该局面不用 TT,无碍。 */

static mut TT: [TtSlot; TT_BUCKETS * 2] = [TtSlot { key: 0, sc: 0, mv: 0, dp: 0, fl: 0 }; TT_BUCKETS * 2];

unsafe fn tt_clear() {
    let tt = &mut *core::ptr::addr_of_mut!(TT);
    for t in tt.iter_mut() {
        *t = TtSlot { key: 0, sc: 0, mv: 0, dp: 0, fl: 0 };
    }
}

/** 探测:命中返回 (桶下标, 命中槽 0/1, 条目副本)。 */
unsafe fn tt_probe(key: u64) -> Option<(usize, usize, TtSlot)> {
    let idx = (key & TT_MASK) as usize;
    let tt = &mut *core::ptr::addr_of_mut!(TT);
    if tt[idx].key == key {
        return Some((idx, 0, tt[idx]));
    }
    if tt[TT_BUCKETS + idx].key == key {
        return Some((idx, 1, tt[TT_BUCKETS + idx]));
    }
    None
}

unsafe fn tt_store(key: u64, idx: usize, sc: i32, dp: i32, fl: u8, mv: i32) {
    let dp = dp.min(127) as i8;
    let e = TtSlot { key, sc, mv, dp, fl };
    let tt = &mut *core::ptr::addr_of_mut!(TT);
    let old = tt[idx];
    if old.key == 0 || dp >= old.dp {
        /* push-down:同键是更新,异键把旧条目压入 always 槽 */
        if old.key != 0 && old.key != key {
            tt[TT_BUCKETS + idx] = old;
        }
        tt[idx] = e;
    } else {
        tt[TT_BUCKETS + idx] = e;
    }
}

/* ==================== 启发式表 ==================== */

static mut KILLER: [[i32; 2]; MAXPLY] = [[-1; 2]; MAXPLY];
static mut HIST: [i32; 2 * SIZE] = [0; 2 * SIZE];
/* 连续历史:1-ply 反着历史 [prev][curr],重力公式
 *  h += b - h*|b|/16384;beta 截断给截断着加分、先试过的安静着减分。 */
const HIST_MAX: i32 = 16384;
static mut CONT1: [i32; SIZE * SIZE] = [0; SIZE * SIZE];

#[inline]
unsafe fn cont1_gravity(slot: &mut i32, bonus: i32) {
    *slot += bonus - (*slot * bonus.abs()) / HIST_MAX;
}

/* ==================== 走法缓冲(每 ply 一段,零分配) ==================== */

pub const MOVE_CAP: usize = SIZE + 1;
static mut MB: [i32; MOVE_CAP * (MAXPLY + 8)] = [0; MOVE_CAP * (MAXPLY + 8)];
static mut MS: [i32; MOVE_CAP * (MAXPLY + 8)] = [0; MOVE_CAP * (MAXPLY + 8)];

#[inline]
unsafe fn mb(base: usize) -> &'static mut [i32] {
    let b = &mut *core::ptr::addr_of_mut!(MB);
    &mut b[base..base + MOVE_CAP]
}
#[inline]
unsafe fn ms(base: usize) -> &'static mut [i32] {
    let b = &mut *core::ptr::addr_of_mut!(MS);
    &mut b[base..base + MOVE_CAP]
}

/* 威胁分级(tier):7 己成五 / 6 堵五 / 5 己双四 /
 * 4 堵双四 / 3 己冲四 / 2 堵冲四 / 0 无。威胁永远排在安静着前。 */
#[inline]
pub unsafe fn tier_of(cell: usize, side: u8) -> i32 {
    let q = p();
    let cn = CWCOUNT[cell] as usize;
    let mut my4 = false;
    let mut op4 = false;
    let mut my3 = 0i32;
    let mut op3 = 0i32;
    let mut k = 0;
    while k < cn {
        let w = CW[cell * 20 + k] as usize;
        let b = q.wb[w] as usize;
        let o = q.ww[w] as usize;
        if b > 0 && o > 0 {
            k += 1;
            continue;
        }
        let (m, oo) = if side == BLACK { (b, o) } else { (o, b) };
        if m == 4 {
            my4 = true;
        } else if oo == 4 {
            op4 = true;
        } else if m == 3 {
            my3 += 1;
        } else if oo == 3 {
            op3 += 1;
        }
        k += 1;
    }
    if my4 {
        7
    } else if op4 {
        6
    } else if my3 >= 2 {
        5
    } else if op3 >= 2 {
        4
    } else if my3 == 1 {
        3
    } else if op3 == 1 {
        2
    } else {
        0
    }
}

/* ==================== 主搜索(PVS) ==================== */

/* 每路径威胁延伸预算(己方造四着不减层;沿路径递减防爆)。
 * 运行时可调:对拍时同进程内让两种配置互弈。
 * 默认 0 = 关闭:实测(原生 320 局)预算 6/12 在 10 万节点/手
 * 下均为净负收益 —— qsearch + 根 VCT 已覆盖强制链,延伸挤占
 * 主迭代深度;机制保留供后续更高节点预算下复测。 */
pub static mut EXT_BUDGET: i32 = 0;

/** 设置延伸预算(0 = 关闭延伸);返回旧值。 */
pub unsafe fn set_ext_budget(b: i32) -> i32 {
    let old = EXT_BUDGET;
    EXT_BUDGET = b;
    old
}

unsafe fn ab(
    side: u8,
    mut depth: i32,
    mut alpha: i32,
    beta: i32,
    ply: usize,
    prev: i32,
    ext_left: i32,
) -> i32 {
    (*s()).nodes += 1;
    if time_up() {
        return 0;
    }
    if (*p()).stones >= SIZE as u16 {
        return 0; // 满盘和棋
    }
    if depth <= 0 {
        let cap = (*s()).qs_cap.max(ply + 14);
        (*s()).qs_cap = cap;
        return qsearch(side, alpha, beta, ply);
    }

    /* 置换表探测 */
    let key = (*p()).hk ^ if side == WHITE { ZSIDE } else { 0 };
    let mut tt_move = -1i32;
    if let Some((_, _, e)) = tt_probe(key) {
        tt_move = e.mv;
        if e.dp as i32 >= depth && ply > 0 {
            if e.fl == 0 {
                return e.sc;
            }
            if e.fl == 2 && e.sc >= beta {
                return e.sc;
            }
            if e.fl == 3 && e.sc <= alpha {
                return e.sc;
            }
        }
    }

    let is_pv = beta - alpha > 1;

    /* ---- IIR:TT 未命中且非 PV 的深节点减 1 层 ---- */
    if depth >= 4 && tt_move < 0 && !is_pv {
        depth -= 1;
    }

    /* 候选生成 + 排序打分 */
    let base = ply * MOVE_CAP;
    let mut n = 0usize;
    {
        let mbuf = mb(base);
        let sbuf = ms(base);
        let q = p();
        if q.stones == 0 {
            /* 空盘:天元附近 9 点,天元优先 */
            let pts: [(i32, i32); 9] = [
                (7, 7), (6, 6), (6, 7), (6, 8), (7, 6), (7, 8), (8, 6), (8, 7), (8, 8),
            ];
            for &(r, c) in pts.iter() {
                mbuf[n] = r * N as i32 + c;
                sbuf[n] = 1 << 20; // 无威胁、增益并列,天元第一
                n += 1;
            }
        } else {
            let mut i = 0;
            while i < SIZE {
                if q.bd[i] == 0 && q.nei[i] > 0 {
                    mbuf[n] = i as i32;
                    n += 1;
                }
                i += 1;
            }
            if n == 0 {
                /* 邻居区无空点(病态孤岛):回退全盘空点,合法性在任何局面成立 */
                let mut i = 0;
                while i < SIZE {
                    if q.bd[i] == 0 {
                        mbuf[n] = i as i32;
                        n += 1;
                    }
                    i += 1;
                }
            }
        }
    }
    /* 打分单独一段(重入 p(),不与生成段的借用重叠) */
    {
        let mbuf = mb(base);
        let sbuf = ms(base);
        let mut j = 0;
        while j < n {
            let mv = mbuf[j] as usize;
            let tier = tier_of(mv, side);
            let mut sc = tier << 24;
            sc += point_score(mv, side).min((1 << 20) - 1);
            if mv as i32 == tt_move {
                sc += 1 << 30;
            } else {
                if ply < MAXPLY {
                    let (k0, k1) = (KILLER[ply][0], KILLER[ply][1]);
                    if k0 == mv as i32 || k1 == mv as i32 {
                        sc += 1 << 23;
                    }
                }
                sc += HIST[side as usize * SIZE + mv].min((1 << 20) - 1);
                if prev >= 0 {
                    let c1 = CONT1[prev as usize * SIZE + mv] >> 2;
                    sc += c1.clamp(-(1 << 19), 1 << 19);
                }
            }
            sbuf[j] = sc;
            j += 1;
        }
    }

    let mut best = -INF;
    let mut best_move = -1i32;
    let mut flag = 3u8;
    let mut any_legal = false;
    let mut searched = 0usize;

    let mut i = 0;
    while i < n {
        /* 选择排序取当前最大分(候选 ≤225,截断早,零分配) */
        {
            let mbuf = mb(base);
            let sbuf = ms(base);
            let mut bi = i;
            let mut j = i + 1;
            while j < n {
                if sbuf[j] > sbuf[bi] {
                    bi = j;
                }
                j += 1;
            }
            if bi != i {
                mbuf.swap(i, bi);
                sbuf.swap(i, bi);
            }
        }
        let (cell, tier, is_killer) = {
            let mbuf = mb(base);
            let sbuf = ms(base);
            let cell = mbuf[i] as usize;
            let tier = (sbuf[i] >> 24) & 7;
            let is_killer =
                ply < MAXPLY && (KILLER[ply][0] == cell as i32 || KILLER[ply][1] == cell as i32);
            (cell, tier, is_killer)
        };
        let forcing = tier >= 2;
        /* 威胁延伸:己方造四着(tier≥3)不减层 —— 四威胁必被应答,
         * 强制链窄;每路径预算控制四点密集局面的树膨胀。 */
        let ext = tier >= 3 && ext_left > 0 && ply < 40;
        let nd = depth - 1 + ext as i32;
        let child_ext = ext_left - ext as i32;

        /* LMP:浅层非 PV 的安静着,试够阈值后整段放弃 */
        if any_legal
            && !is_pv
            && !forcing
            && !is_killer
            && depth <= 3
            && searched >= (8 + 4 * depth) as usize
        {
            break;
        }

        if (*s()).mode == RENJU && side == BLACK && is_forbidden(cell, 0) {
            i += 1;
            continue;
        }
        any_legal = true;
        make(cell, side);
        let sc;
        if made_five(cell, (*s()).mode) {
            sc = MATE - ply as i32 - 1;
        } else if searched == 0 {
            sc = -ab(side ^ 1, nd, -beta, -alpha, ply + 1, cell as i32, child_ext);
        } else {
            /* LMR:零窗口试探 → 全深零窗口 → 全窗口重搜 */
            let mut r = 0i32;
            if depth >= 3 && searched >= 3 && !forcing && !is_killer {
                r = 1 + if depth >= 5 { 1 } else { 0 } + if searched >= 8 { 1 } else { 0 };
                r = r.min(nd - 1).max(0);
            }
            let mut v = -ab(
                side ^ 1,
                nd - r,
                -alpha - 1,
                -alpha,
                ply + 1,
                cell as i32,
                child_ext,
            );
            if !(*s()).aborted && r > 0 && v > alpha {
                v = -ab(side ^ 1, nd, -alpha - 1, -alpha, ply + 1, cell as i32, child_ext);
            }
            if !(*s()).aborted && v > alpha && v < beta {
                v = -ab(side ^ 1, nd, -beta, -alpha, ply + 1, cell as i32, child_ext);
            }
            sc = v;
        }
        unmake(cell, side);
        if (*s()).aborted {
            return 0;
        }
        if sc > best {
            best = sc;
            best_move = cell as i32;
            if sc > alpha {
                alpha = sc;
                flag = 0;
            }
            if alpha >= beta {
                flag = 2;
                if ply < MAXPLY && KILLER[ply][0] != cell as i32 {
                    KILLER[ply][1] = KILLER[ply][0];
                    KILLER[ply][0] = cell as i32;
                }
                HIST[side as usize * SIZE + cell] += depth * depth;
                if !forcing && prev >= 0 {
                    let slot = &mut CONT1[prev as usize * SIZE + cell];
                    cont1_gravity(slot, (depth * depth).min(HIST_MAX));
                }
                break;
            }
        }
        searched += 1;
        i += 1;
    }

    if !any_legal {
        /* 黑棋候选区全被禁手封死 = 必负;白方候选区为空即满盘 */
        if (*s()).mode == RENJU && side == BLACK {
            return -MATE + ply as i32;
        }
        return 0;
    }
    tt_store(key, (key & TT_MASK) as usize, best, depth, flag, best_move);
    best
}

/* 静态搜索(VCF 式分级):己方成五点先走 → 对方叫五必堵 →
 * 己方成四叫杀;严格交替,分支恒小。 */
unsafe fn qsearch(side: u8, mut alpha: i32, beta: i32, ply: usize) -> i32 {
    (*s()).nodes += 1;
    if time_up() {
        return 0;
    }
    let mut best = evaluate(side, (*s()).mode);
    if best >= beta {
        return best;
    }
    if best > alpha {
        alpha = best;
    }
    if ply >= MAXPLY - 4 || ply >= (*s()).qs_cap {
        return best;
    }

    /* O(1) 快筛:my4 / op4 / my3 = 纯色窗计数 */
    let (my4, op4, my3) = {
        let q = p();
        let (a, b, c) = if side == BLACK {
            (q.cnt[4 * WSC_DIM], q.cnt[4], q.cnt[3 * WSC_DIM])
        } else {
            (q.cnt[4], q.cnt[4 * WSC_DIM], q.cnt[3])
        };
        (a, b, c)
    };
    if my4 == 0 && op4 == 0 && my3 == 0 {
        return best;
    }

    let base = ply * MOVE_CAP;
    let mut n = 0usize;
    {
        let mbuf = mb(base);
        let sbuf = ms(base);
        let q = p();
        let mut i = 0;
        while i < SIZE {
            if q.bd[i] != 0 || q.nei[i] == 0 {
                i += 1;
                continue;
            }
            let cn = CWCOUNT[i] as usize;
            let mut ok = false;
            let mut k = 0;
            while k < cn && !ok {
                let w = CW[i * 20 + k] as usize;
                let b = q.wb[w] as usize;
                let o = q.ww[w] as usize;
                if b > 0 && o > 0 {
                    k += 1;
                    continue;
                }
                ok = if my4 > 0 {
                    if side == BLACK { b == 4 && o == 0 } else { o == 4 && b == 0 }
                } else if op4 > 0 {
                    if side == BLACK { o == 4 && b == 0 } else { b == 4 && o == 0 }
                } else {
                    if side == BLACK { b == 3 && o == 0 } else { o == 3 && b == 0 }
                };
                k += 1;
            }
            if ok {
                mbuf[n] = i as i32;
                sbuf[n] = 0;
                n += 1;
            }
            i += 1;
        }
    }
    if n == 0 {
        return best;
    }
    /* 打分(重入 p(),单独一段) */
    {
        let mbuf = mb(base);
        let sbuf = ms(base);
        let mut j = 0;
        while j < n {
            sbuf[j] = point_score(mbuf[j] as usize, side);
            j += 1;
        }
    }

    let mut i = 0;
    while i < n {
        {
            let mbuf = mb(base);
            let sbuf = ms(base);
            let mut bi = i;
            let mut j = i + 1;
            while j < n {
                if sbuf[j] > sbuf[bi] {
                    bi = j;
                }
                j += 1;
            }
            if bi != i {
                mbuf.swap(i, bi);
                sbuf.swap(i, bi);
            }
        }
        let cell = mb(base)[i] as usize;
        if (*s()).mode == RENJU && side == BLACK && is_forbidden(cell, 0) {
            i += 1;
            continue;
        }
        make(cell, side);
        let sc = if made_five(cell, (*s()).mode) {
            MATE - ply as i32 - 1
        } else {
            -qsearch(side ^ 1, -beta, -alpha, ply + 1)
        };
        unmake(cell, side);
        if (*s()).aborted {
            return 0;
        }
        if sc > best {
            best = sc;
            if sc > alpha {
                alpha = sc;
                if alpha >= beta {
                    break;
                }
            }
        }
        i += 1;
    }
    best
}

/* 根搜索:迭代加深 + 期望窗口 */

const ASPIRATION_MIN_DEPTH: i32 = 3;
const ASPIRATION_DELTA: i32 = 240;

/**
 * 搜索最佳落点(当前活跃局面,行棋方 = stones 奇偶)。
 * 结果写入 S(best / best_score / done_depth / mate / only / draw /
 * root_moves + root_scores + root_len 供低难度随机化)。
 */
pub unsafe fn search_best(mode: i32, max_depth: i32, node_budget: u64, ms_budget: u64) {
    eval::set_mode(mode);
    {
        let st = s();
        (*st).nodes = 0;
        (*st).node_budget = node_budget;
        (*st).ms_budget = ms_budget;
        (*st).t0 = now_ms();
        (*st).aborted = false;
        (*st).qs_cap = 14;
        (*st).mode = mode;
        (*st).best = -1;
        (*st).best_score = 0;
        (*st).done_depth = 0;
        (*st).mate = false;
        (*st).only = false;
        (*st).draw = false;
        (*st).root_len = 0;
    }
    tt_clear();
    for k in &mut *core::ptr::addr_of_mut!(KILLER) {
        *k = [-1; 2];
    }
    unsafe fn zero<T: Copy>(p: *mut T, v: T, n: usize) {
        let s = core::slice::from_raw_parts_mut(p, n);
        for x in s.iter_mut() {
            *x = v;
        }
    }
    zero(core::ptr::addr_of_mut!(HIST) as *mut i32, 0, 2 * SIZE);
    zero(core::ptr::addr_of_mut!(CONT1) as *mut i32, 0, SIZE * SIZE);

    let side = side_to_move();
    let mut cells = [0i32; SIZE + 1];
    let n = legal_moves(&mut cells, side, mode);

    if n == 0 {
        let lost = mode == RENJU && side == BLACK && ((*p()).stones as usize) < SIZE;
        (*s()).best = -1;
        (*s()).best_score = if lost { -MATE } else { 0 };
        (*s()).mate = lost;
        (*s()).draw = !lost;
        return;
    }
    if n == 1 {
        (*s()).best = cells[0];
        (*s()).done_depth = 1;
        (*s()).only = true;
        return;
    }

    /* ---- 根 VCT 证明搜索:预算 1/8,深度 12 ---- */
    {
        let st = s();
        (*st).node_budget = (*st).nodes + (node_budget / 8).clamp(1000, 80000);
    }
    let vct_first = vct::search(side, mode, 12);
    let vct_nodes = (*s()).nodes;
    {
        let st = s();
        (*st).nodes = 0;
        (*st).node_budget = node_budget;
        (*st).aborted = false; // VCT 只耗自己的预算;命中则下面回报其节点数
    }
    if let Some(mv) = vct_first {
        let st = s();
        (*st).nodes = vct_nodes;
        (*st).best = mv;
        (*st).best_score = MATE - 1;
        (*st).done_depth = 12;
        (*st).mate = true;
        (*st).root_len = 1;
        (*st).root_moves[0] = mv;
        (*st).root_scores[0] = MATE - 1;
        return;
    }

    /* 根着法表:首轮按 tier 排序,后续轮按上轮得分排序 */
    let mut rm = [0i32; SIZE + 1];
    let mut rs = [0i32; SIZE + 1];
    let mut order = [0usize; SIZE + 1];
    let mut i = 0;
    while i < n {
        order[i] = i;
        rm[i] = cells[i];
        rs[i] = tier_of(cells[i] as usize, side) << 24;
        i += 1;
    }
    let mut i = 1;
    while i < n {
        let mut j = i;
        while j > 0 && rs[order[j]] > rs[order[j - 1]] {
            order.swap(j, j - 1);
            j -= 1;
        }
        i += 1;
    }

    let mut best_idx = order[0];
    let mut best_score = 0i32;
    let mut done_depth = 0i32;
    let mut scores = [0i32; SIZE + 1]; // cells 下标 → 分

    let mut depth = 1;
    while depth <= max_depth {
        /* 期望窗口:上轮分数 ±δ;fail-low/high 时倍增扩窗重搜 */
        let mut alpha;
        let mut beta;
        let mut delta = ASPIRATION_DELTA;
        if depth >= ASPIRATION_MIN_DEPTH && done_depth > 0 {
            alpha = best_score - delta;
            beta = best_score + delta;
        } else {
            alpha = -INF;
            beta = INF;
        }

        let (mut iter_best, mut iter_score) = (best_idx, best_score);
        loop {
            let mut a = alpha;
            let b = beta;
            let mut local_best = 0usize;
            let mut local_score = -INF;
            let mut k = 0;
            while k < n {
                let idx = order[k];
                let cell = rm[idx] as usize;
                /* 根处延伸与树内同规则(tier 随深度迭代被覆盖,逐手重算);
                 * 根每轮只延伸首个强制着 */
                let ext = EXT_BUDGET > 0 && tier_of(cell, side) >= 3;
                let nd = depth - 1 + ext as i32;
                let child_ext = EXT_BUDGET - ext as i32;
                make(cell, side);
                let sc;
                if made_five(cell, mode) {
                    sc = MATE - 1;
                } else if k == 0 {
                    sc = -ab(side ^ 1, nd, -b, -a, 1, cell as i32, child_ext);
                } else {
                    /* 根 PVS:零窗口试探,failed-high 再全窗口确认 */
                    let mut v = -ab(side ^ 1, nd, -a - 1, -a, 1, cell as i32, child_ext);
                    if !(*s()).aborted && v > a && v < b {
                        v = -ab(side ^ 1, nd, -b, -a, 1, cell as i32, child_ext);
                    }
                    sc = v;
                }
                unmake(cell, side);
                if (*s()).aborted {
                    break;
                }
                scores[idx] = sc;
                if sc > local_score {
                    local_score = sc;
                    local_best = idx;
                }
                if sc > a {
                    a = sc;
                }
                k += 1;
            }
            if (*s()).aborted {
                break;
            }
            if local_score >= b && b < INF {
                delta *= 2; // fail-high:分数只是下界,扩窗重搜
                beta = (beta + delta).min(INF);
                continue;
            }
            if local_score <= alpha && alpha > -INF {
                delta *= 2; // fail-low:上轮最好着被推翻,向下扩窗重搜
                alpha = (alpha - delta).max(-INF);
                continue;
            }
            iter_best = local_best;
            iter_score = local_score;
            break;
        }
        if (*s()).aborted {
            break;
        }

        best_idx = iter_best;
        best_score = iter_score;
        done_depth = depth;
        /* 上一层分数当下一层排序(插入排序,同分稳定) */
        let mut i = 1;
        while i < n {
            let mut j = i;
            while j > 0 && scores[order[j]] > scores[order[j - 1]] {
                order.swap(j, j - 1);
                j -= 1;
            }
            i += 1;
        }
        if best_score.abs() > MATE - 200 {
            break;
        }
        if now_ms() - (*s()).t0 > (*s()).ms_budget as f64 {
            break;
        }
        depth += 1;
    }

    /* 中止时保留上一完成轮的结果(部分轮分数仅用于随机化池) */
    {
        let st = s();
        if done_depth == 0 {
            (*st).best = rm[order[0]]; // 一轮没完成:排序最高候选,保证有子可下
            (*st).best_score = 0;
        } else {
            (*st).best = rm[best_idx];
            (*st).best_score = best_score;
            (*st).mate = best_score.abs() > MATE - 200;
        }
        (*st).done_depth = done_depth;
        let mut k = 0;
        while k < n {
            (*st).root_moves[k] = rm[order[k]];
            (*st).root_scores[k] = scores[order[k]];
            k += 1;
        }
        (*st).root_len = n;
    }
}
