/* ============================================================
 * AetherRenju 引擎核心(Rust):规则 + 评估 + 禁手。
 * 15×15,idx = 行×15+列;格值 0 空/1 黑/2 白;黑先白后。
 * FREE 无禁(≥5 胜) / RENJU 有禁(RIF:黑长连/双四/双活三负,
 * 恰好五连才胜,白无限制)。RIF 禁手推导见 is_forbidden 注释。

 * wasm32 上 no_std、零动态分配(见 api.rs)。
 * ============================================================ */
#![cfg_attr(target_arch = "wasm32", no_std)]

/* no_std(wasm)下的 panic 处理:引擎内部不可能 panic(全部整数运算、
 * 有界索引),这个 handler 只是满足 lang item 要求。 */
#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

pub mod api;
pub mod eval;
pub mod opening;
pub mod search;
pub mod vct;

pub const N: usize = 15;
pub const SIZE: usize = 225;
pub const BLACK: u8 = 0;
pub const WHITE: u8 = 1;
pub const FREE: i32 = 0;
pub const RENJU: i32 = 1;
pub const MATE: i32 = 30000;
pub const INF: i32 = 1 << 28;
pub const MAXPLY: usize = 96;

pub const DIRS: usize = 4;
pub const DR: [i32; 4] = [0, 1, 1, 1];
pub const DC: [i32; 4] = [1, 0, 1, -1];
pub const STRIDE: [usize; 4] = [1, N, N + 1, N - 1];

/* 常量表(const 求值,零初始化开销) */

/* 邻居表:切比雪夫距离 ≤2 的 24 个邻点,-1 = 盘外。候选点生成用:
 * NEI[q] > 0 的空点才是「有意义的落点」。 */
const fn gen_nbl() -> [i16; SIZE * 24] {
    let mut t = [-1i16; SIZE * 24];
    let mut i = 0;
    while i < SIZE {
        let r = (i / N) as i32;
        let c = (i % N) as i32;
        let mut m = 0;
        let mut dr = -2;
        while dr <= 2 {
            let mut dc = -2;
            while dc <= 2 {
                if dr != 0 || dc != 0 {
                    let rr = r + dr;
                    let cc = c + dc;
                    if rr >= 0 && rr < N as i32 && cc >= 0 && cc < N as i32 {
                        t[i * 24 + m] = (rr * N as i32 + cc) as i16;
                    }
                    m += 1;
                }
                dc += 1;
            }
            dr += 1;
        }
        i += 1;
    }
    t
}
pub const NBL: [i16; SIZE * 24] = gen_nbl();

/* 五元窗:4 个方向上所有长度 5 的连续线段,共 572 个。
 * WCELLS:窗 → 5 个点;CW / CWCOUNT:点 → 所属窗(每方向至多 5 个)。 */
pub const NW: usize = 572;

const fn gen_windows() -> ([u16; NW * 5], [u16; SIZE * 20], [u8; SIZE]) {
    let mut wc = [0u16; NW * 5];
    let mut cw = [0u16; SIZE * 20];
    let mut cc = [0u8; SIZE];
    let mut w = 0usize;
    let mut d = 0;
    while d < DIRS {
        let dr = DR[d];
        let dc = DC[d];
        let mut r = 0;
        while r < N as i32 {
            let mut c = 0;
            while c < N as i32 {
                let er = r + dr * 4;
                let ec = c + dc * 4;
                if er >= 0 && er < N as i32 && ec >= 0 && ec < N as i32 {
                    let mut k = 0usize;
                    while k < 5 {
                        let cell = ((r + dr * k as i32) * N as i32 + (c + dc * k as i32)) as usize;
                        wc[w * 5 + k] = cell as u16;
                        cw[cell * 20 + cc[cell] as usize] = w as u16;
                        cc[cell] += 1;
                        k += 1;
                    }
                    w += 1;
                }
                c += 1;
            }
            r += 1;
        }
        d += 1;
    }
    (wc, cw, cc)
}
pub const WCELLS: [u16; NW * 5] = gen_windows().0;
pub const CW: [u16; SIZE * 20] = gen_windows().1;
pub const CWCOUNT: [u8; SIZE] = gen_windows().2;

/* 窗分权重:仅用于走法排序的落点增益(point_score),静态评估在
 * eval.rs(线形分类 + 权重表);混色 0 分、纯黑 +W[b]、纯白 −W[w]。
 * W[5]/W7[6] 是终局瞬态钳制值,保证 make/unmake 增量对称。 */
pub const W: [i32; 6] = [0, 4, 36, 320, 2800, 1200000];
pub const W7: [i32; 7] = [0, 4, 36, 320, 2800, 1200000, 1200000];
pub const WSC_DIM: usize = 7;
pub const WSC_LEN: usize = 49;

const fn gen_wsc() -> [i32; 49] {
    let mut s = [0i32; 49];
    let mut b = 0;
    while b <= 6 {
        let mut o = 0;
        while o <= 6 {
            s[b * WSC_DIM + o] = if b > 0 && o > 0 {
                0
            } else if b > 0 {
                W7[b]
            } else if o > 0 {
                -W7[o]
            } else {
                0
            };
            o += 1;
        }
        b += 1;
    }
    s
}
pub const WSC: [i32; 49] = gen_wsc();

/* Zobrist(splitmix64 固定种子,全平台一致) */
const fn mix(s: u64) -> u64 {
    let mut z = s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
const fn gen_zob() -> [u64; 2 * SIZE] {
    let mut z = [0u64; 2 * SIZE];
    let mut s = 0x51F2_A3B7_C0FF_EE01u64;
    let mut i = 0;
    while i < 2 * SIZE {
        s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z[i] = mix(s);
        i += 1;
    }
    z
}
pub const ZOB: [u64; 2 * SIZE] = gen_zob();
pub const ZSIDE: u64 = mix(0x9E37_79B9_7F4A_7C15);

/* ==================== 活跃局面(单一全局,单线程) ==================== */

pub struct Pos {
    pub bd: [u8; SIZE],
    pub hk: u64,       // Zobrist 键(只含子力;行棋方由 stones 奇偶给出)
    pub stones: u16,
    pub nei: [u16; SIZE], // 邻居计数
    pub wb: [u8; NW],
    pub ww: [u8; NW],
    pub cnt: [i32; 49],  // (b,w) → 窗口数(qsearch O(1) 快筛)
}

const fn zero_cnt() -> [i32; 49] {
    let mut c = [0i32; 49];
    c[0] = NW as i32;
    c
}

pub static mut P: Pos = Pos {
    bd: [0; SIZE],
    hk: 0,
    stones: 0,
    nei: [0; SIZE],
    wb: [0; NW],
    ww: [0; NW],
    cnt: zero_cnt(),
};

#[inline]
pub unsafe fn p() -> &'static mut Pos {
    &mut *core::ptr::addr_of_mut!(P)
}

/* ==================== 局面构建 ==================== */

/** 空盘(黑先);重算全部派生状态 */
pub unsafe fn new_board() {
    let q = p();
    q.bd = [0; SIZE];
    q.hk = 0;
    q.stones = 0;
    q.nei = [0; SIZE];
    q.wb = [0; NW];
    q.ww = [0; NW];
    q.cnt = zero_cnt(); // zero_cnt 已含 cnt[0] = NW(空盘所有窗都是 (0,0))
    eval::reset();
}

/** 摆棋后重算全部派生状态(测试 / UI 侧构造局面用) */
pub unsafe fn sync_position(bd: &[u8; SIZE]) {
    new_board();
    let mut i = 0;
    while i < SIZE {
        if bd[i] != 0 {
            make(i, (bd[i] - 1) as u8);
        }
        i += 1;
    }
}

#[inline]
pub unsafe fn side_to_move() -> u8 {
    if p().stones % 2 == 0 { BLACK } else { WHITE }
}

/** 从空盘按落点序列重演(Worker 还原 UI 棋盘用)。
 *  返回:-1 序列非法;-2 末手成五(有效终局);否则 = 轮到谁。 */
pub unsafe fn replay_moves(moves: &[u8], mode: i32) -> i32 {
    eval::set_mode(mode);
    let mut s = BLACK;
    let mut i = 0;
    while i < moves.len() {
        let cell = moves[i] as usize;
        if cell >= SIZE || p().bd[cell] != 0 {
            return -1;
        }
        if mode == RENJU && s == BLACK && is_forbidden(cell, 0) {
            return -1;
        }
        make(cell, s);
        if made_five(cell, mode) {
            return if i == moves.len() - 1 { -2 } else { -1 };
        }
        s ^= 1;
        i += 1;
    }
    s as i32
}

/* ==================== 走子(增量维护全部派生状态) ==================== */

pub unsafe fn make(cell: usize, side: u8) {
    let q = p();
    q.bd[cell] = side + 1;
    q.stones += 1;
    q.hk ^= ZOB[side as usize * SIZE + cell];
    let mut k = 0;
    while k < 24 {
        let nb = NBL[cell * 24 + k];
        if nb >= 0 {
            q.nei[nb as usize] += 1;
        }
        k += 1;
    }
    let cn = CWCOUNT[cell] as usize;
    let mut k = 0;
    while k < cn {
        let w = CW[cell * 20 + k] as usize;
        let b = q.wb[w] as usize;
        let o = q.ww[w] as usize;
        q.cnt[b * WSC_DIM + o] -= 1;
        if side == BLACK {
            q.wb[w] += 1;
            q.cnt[(b + 1) * WSC_DIM + o] += 1;
        } else {
            q.ww[w] += 1;
            q.cnt[b * WSC_DIM + o + 1] += 1;
        }
        k += 1;
    }
    eval::after_stone_change(cell, true);
    eval::snapshot();
}

/** 撤销:异或与计数自逆,与 make 完全镜像 */
pub unsafe fn unmake(cell: usize, side: u8) {
    let q = p();
    q.bd[cell] = 0;
    q.stones -= 1;
    q.hk ^= ZOB[side as usize * SIZE + cell];
    let mut k = 0;
    while k < 24 {
        let nb = NBL[cell * 24 + k];
        if nb >= 0 {
            q.nei[nb as usize] -= 1;
        }
        k += 1;
    }
    let cn = CWCOUNT[cell] as usize;
    let mut k = 0;
    while k < cn {
        let w = CW[cell * 20 + k] as usize;
        let b = q.wb[w] as usize;
        let o = q.ww[w] as usize;
        q.cnt[b * WSC_DIM + o] -= 1;
        if side == BLACK {
            q.wb[w] -= 1;
            q.cnt[(b - 1) * WSC_DIM + o] += 1;
        } else {
            q.ww[w] -= 1;
            q.cnt[b * WSC_DIM + o - 1] += 1;
        }
        k += 1;
    }
    eval::after_stone_change(cell, false);
}

/* ==================== 胜负判定 ==================== */

/** cell(已落)沿 dir 的连长(含自身) */
pub unsafe fn run_len(cell: usize, pv: u8, d: usize) -> i32 {
    let q = p();
    let r0 = (cell / N) as i32;
    let c0 = (cell % N) as i32;
    let mut n = 1;
    let mut s = -1;
    while s <= 1 {
        let mut r = r0 + DR[d] * s;
        let mut c = c0 + DC[d] * s;
        while r >= 0 && r < N as i32 && c >= 0 && c < N as i32 && q.bd[(r * N as i32 + c) as usize] == pv {
            n += 1;
            r += DR[d] * s;
            c += DC[d] * s;
        }
        s += 2;
    }
    n
}

/** cell(已落)是否成五。有禁黑方必须恰好五连;其余 ≥5 即胜。 */
pub unsafe fn made_five(cell: usize, mode: i32) -> bool {
    let q = p();
    let pv = q.bd[cell];
    if pv == 0 {
        return false;
    }
    let side = pv - 1;
    let exact = mode == RENJU && side == BLACK;
    let mut d = 0;
    while d < DIRS {
        let l = run_len(cell, pv, d);
        if if exact { l == 5 } else { l >= 5 } {
            return true;
        }
        d += 1;
    }
    false
}

/** cell(已落)的获胜连线写入 out(从线段一端到另一端,含 cell),
 *  返回长度(0 = 没赢)。UI 高亮用。 */
pub unsafe fn check_win(cell: usize, mode: i32, out: &mut [usize]) -> usize {
    let q = p();
    let pv = q.bd[cell];
    if pv == 0 {
        return 0;
    }
    let side = pv - 1;
    let exact = mode == RENJU && side == BLACK;
    let r0 = (cell / N) as i32;
    let c0 = (cell % N) as i32;
    let mut d = 0;
    while d < DIRS {
        let mut back = [0usize; 7]; // 反向(由近到远),7+7+1 覆盖整线最长 15
        let mut bn = 0usize;
        let mut fwd = [0usize; 7];  // 正向(由近到远)
        let mut fn_ = 0usize;
        let mut s = -1;
        while s <= 1 {
            let mut r = r0 + DR[d] * s;
            let mut c = c0 + DC[d] * s;
            while r >= 0 && r < N as i32 && c >= 0 && c < N as i32
                && q.bd[(r * N as i32 + c) as usize] == pv
            {
                let idx = (r * N as i32 + c) as usize;
                if s < 0 && bn < 7 {
                    back[bn] = idx;
                    bn += 1;
                } else if s > 0 && fn_ < 7 {
                    fwd[fn_] = idx;
                    fn_ += 1;
                }
                r += DR[d] * s;
                c += DC[d] * s;
            }
            s += 2;
        }
        let total = bn + fn_ + 1;
        if if exact { total == 5 } else { total >= 5 } {
            let mut len = 0;
            let mut i = bn;
            while i > 0 {
                out[len] = back[i - 1];
                len += 1;
                i -= 1;
            }
            out[len] = cell;
            len += 1;
            let mut i = 0;
            while i < fn_ {
                out[len] = fwd[i];
                len += 1;
                i += 1;
            }
            return len;
        }
        d += 1;
    }
    0
}

/* ==================== 禁手(有禁模式,黑方) ====================
 * RIF 规则的递归实现。记落点为 x(先假设黑下在 x):
 *   五连    恰好五连 → 直接获胜,禁手全部豁免(优先级最高)。
 *   长连    ≥6 连 → 禁手。
 *   四      补一子能成「恰好五」的形状;同一方向按成五窗的
 *           4 子集合去重(活四的两个补五点算同一个四)。
 *   三      存在空点 p,补 p 后能形成「过 x 且过 p 的活四」,
 *           且补 p 本身不成五、不是禁手(递归)——假活三不算三。
 *   双四 / 双活三 → 禁手。
 * 全部检查在以 x 为中心的 ±5 线段缓冲上进行,盘外按「白子」处理。 */

static mut LBUF: [u8; 11] = [0; 11]; // ±5 线段缓冲,下标 5 = 中心
static mut PSNAPS: [[u8; 11]; 12] = [[0; 11]; 12]; // p 候选快照(按递归深度分槽)
static mut FSETS: [u8; 20] = [0; 20]; // fourCountDir 成五窗子集合去重区
static mut CURSET: [u8; 4] = [0; 4];

unsafe fn fill_line(cell: usize, d: usize) {
    let q = p();
    let r0 = (cell / N) as i32;
    let c0 = (cell % N) as i32;
    let mut k = -5;
    while k <= 5 {
        let rr = r0 + DR[d] * k;
        let cc = c0 + DC[d] * k;
        LBUF[(k + 5) as usize] = if rr >= 0 && rr < N as i32 && cc >= 0 && cc < N as i32 {
            q.bd[(rr * N as i32 + cc) as usize]
        } else {
            2
        };
        k += 1;
    }
}

/** 中心落黑后,dir 上「经过中心的四」的个数(0/1/2,按成五窗子集合去重) */
unsafe fn four_count_dir(cell: usize, d: usize) -> u8 {
    fill_line(cell, d);
    let mut sets = 0u8;
    let mut k = 1;
    while k <= 5 {
        let mut b = 0u8;
        let mut qv: i32 = -1;
        let mut dead = false;
        let mut i = k;
        while i <= k + 4 {
            let v = LBUF[i];
            if v == 2 {
                dead = true;
                break;
            }
            if v == 1 {
                b += 1;
            } else {
                qv = i as i32;
            }
            i += 1;
        }
        if !dead && b == 4 && qv >= 0 && LBUF[k - 1] != 1 && LBUF[k + 5] != 1 {
            /* 成五窗的 4 个子(去掉补点 q)作为这个四的指纹,去重 */
            let mut m = 0;
            let mut i = k;
            while i <= k + 4 {
                if i as i32 != qv {
                    CURSET[m] = i as u8;
                    m += 1;
                }
                i += 1;
            }
            let mut same = false;
            let mut s = 0;
            while s < sets as usize {
                let mut hit = true;
                let mut j = 0;
                while j < 4 {
                    if FSETS[s * 4 + j] != CURSET[j] {
                        hit = false;
                        break;
                    }
                    j += 1;
                }
                if hit {
                    same = true;
                    break;
                }
                s += 1;
            }
            if !same {
                let mut j = 0;
                while j < 4 {
                    FSETS[sets as usize * 4 + j] = CURSET[j];
                    j += 1;
                }
                sets += 1;
            }
        }
        k += 1;
    }
    sets
}

/** dir 上是否存在「真活三」:存在空点 p,补 p 后出现过中心且过 p 的
 *  活四,p 本身不成五(≤4 连)且不是禁手(递归)。 */
unsafe fn has_real_three(cell: usize, d: usize, depth: usize) -> bool {
    fill_line(cell, d);
    /* 候选点先快照:下面的递归会重填 LBUF(裸指针拷贝,避开静态区引用) */
    let (src_p, dst_p) = (
        core::ptr::addr_of!(LBUF) as *const u8,
        core::ptr::addr_of_mut!(PSNAPS) as *mut u8,
    );
    core::ptr::copy_nonoverlapping(src_p, dst_p.add(depth * 11), 11);
    let stride = STRIDE[d];
    let mut pi = 2;
    while pi <= 8 {
        if pi != 5 && PSNAPS[depth][pi] == 0 {
            let pcell = (cell as i64 + (pi as i64 - 5) * stride as i64) as usize;
            make(pcell, BLACK);
            fill_line(cell, d);
            let mut ok = false;
            /* 活四 = 恰好 4 连,含中心(5)与 p(pi),两端空、两端外沿无黑 */
            let mut s = 2;
            while s <= 5 && !ok {
                if pi >= s && pi <= s + 3 {
                    let mut all = true;
                    let mut i = s;
                    while i <= s + 3 {
                        if LBUF[i] != 1 {
                            all = false;
                            break;
                        }
                        i += 1;
                    }
                    if all && LBUF[s - 1] == 0 && LBUF[s + 4] == 0 && LBUF[s - 2] != 1 && LBUF[s + 5] != 1 {
                        ok = true;
                    }
                }
                s += 1;
            }
            if ok {
                /* 补 p 不得同时成五/长连(RIF 三的定义) */
                let mut dd = 0;
                while dd < DIRS && ok {
                    if run_len(pcell, 1, dd) >= 5 {
                        ok = false;
                    }
                    dd += 1;
                }
            }
            unmake(pcell, BLACK);
            if ok {
                ok = !is_forbidden(pcell, depth + 1);
            }
            if ok {
                return true;
            }
        }
        pi += 1;
    }
    false
}

/* 禁手判定缓存:键 = 盘面键 ⊕ 该点黑子 Zobrist(纯几何性质,与轮谁无关) */
const FB_BITS: u32 = 14;
const FB_SIZE: usize = 1 << FB_BITS;
const FB_MASK: u64 = (FB_SIZE - 1) as u64;
static mut FBK: [u64; FB_SIZE] = [0; FB_SIZE];
static mut FBV: [u8; FB_SIZE] = [0; FB_SIZE];

/** 有禁模式下,黑棋下在空点 cell 是否禁手(长连 / 双四 / 双活三)。
 *  五连优先:同时成五则不是禁手。depth 只作递归保险(>8 视为禁手)。 */
pub unsafe fn is_forbidden(cell: usize, depth: usize) -> bool {
    if depth > 8 {
        return true;
    }
    let q = p();
    let k = q.hk ^ ZOB[cell];
    let fbi = (k & FB_MASK) as usize;
    if FBK[fbi] == k && FBV[fbi] != 0 {
        /* FBV:1 = 禁手,2 = 合法,0 = 空(避免与「合法」的 0 混淆) */
        return FBV[fbi] == 1;
    }
    make(cell, BLACK);
    let mut five = false;
    let mut over = false;
    let mut d = 0;
    while d < DIRS {
        let l = run_len(cell, 1, d);
        if l == 5 {
            five = true;
            break;
        }
        if l >= 6 {
            over = true;
        }
        d += 1;
    }
    let forbidden;
    if five {
        forbidden = false;
    } else if over {
        forbidden = true;
    } else {
        let mut fours = 0;
        let mut d = 0;
        while d < DIRS {
            fours += four_count_dir(cell, d) as i32;
            if fours >= 2 {
                break;
            }
            d += 1;
        }
        if fours >= 2 {
            forbidden = true;
        } else {
            let mut threes = 0;
            let mut d = 0;
            while d < DIRS {
                if has_real_three(cell, d, depth) {
                    threes += 1;
                    if threes >= 2 {
                        break;
                    }
                }
                d += 1;
            }
            forbidden = threes >= 2;
        }
    }
    unmake(cell, BLACK);
    FBK[fbi] = k;
    FBV[fbi] = if forbidden { 1 } else { 2 };
    forbidden
}

/** 有禁模式下黑方的全部禁手点(UI 画 × 用);写入 out,返回个数 */
pub unsafe fn forbidden_points(out: &mut [usize]) -> usize {
    /* 先快照空点/邻居状态(is_forbidden 会重入 p(),不与其共享借用) */
    let mut cand = [false; SIZE];
    {
        let q = p();
        let mut i = 0;
        while i < SIZE {
            cand[i] = q.bd[i] == 0 && q.nei[i] > 0;
            i += 1;
        }
    }
    let mut n = 0;
    let mut i = 0;
    while i < SIZE {
        if cand[i] && is_forbidden(i, 0) {
            out[n] = i;
            n += 1;
        }
        i += 1;
    }
    n
}

/* ==================== 候选点与合法着法 ==================== */

/** 候选点写入 mb(调用方提供缓冲),返回个数:
 *  有邻居的空点;空盘给天元附近 9 点;邻居区无空点时回退全盘空点。
 *  ms 若非空则写排序分(落点增益);调用方传空切片可跳过打分。 */
pub unsafe fn gen_candidates(side: u8, mb: &mut [i32], ms: &mut [i32]) -> usize {
    let mut cells = [0usize; SIZE + 1];
    let mut n = 0usize;
    {
        let q = p();
        if q.stones == 0 {
            /* 空盘:天元附近 9 点,天元优先(评估并列时第一候选胜出) */
            let pts: [(i32, i32); 9] = [
                (7, 7), (6, 6), (6, 7), (6, 8), (7, 6), (7, 8), (8, 6), (8, 7), (8, 8),
            ];
            for &(r, c) in pts.iter() {
                cells[n] = (r * N as i32 + c) as usize;
                n += 1;
            }
            if !ms.is_empty() {
                let mut i = 0;
                while i < n {
                    let (r, c) = ((cells[i] / N) as i32, (cells[i] % N) as i32);
                    mb[i] = cells[i] as i32;
                    ms[i] = 9 - (r - 7).abs().max((c - 7).abs());
                    i += 1;
                }
            }
            return n;
        }
        let mut i = 0;
        while i < SIZE {
            if q.bd[i] == 0 && q.nei[i] > 0 {
                cells[n] = i;
                n += 1;
            }
            i += 1;
        }
        if n == 0 {
            let mut i = 0;
            while i < SIZE {
                if q.bd[i] == 0 {
                    cells[n] = i;
                    n += 1;
                }
                i += 1;
            }
        }
    }
    /* 打分单独一段:point_score 重入 p(),不与上面的借用重叠 */
    if !ms.is_empty() {
        let mut i = 0;
        while i < n {
            mb[i] = cells[i] as i32;
            ms[i] = point_score(cells[i], side);
            i += 1;
        }
    } else {
        let mut i = 0;
        while i < n {
            mb[i] = cells[i] as i32;
            i += 1;
        }
    }
    n
}

/** 落点增益 = 若此手落下全盘分的变化量。评估零和对称,
 *  「挡对方的四」与「自己成四」都自然变成大正分,攻防一体。 */
pub unsafe fn point_score(cell: usize, side: u8) -> i32 {
    let q = p();
    let cn = CWCOUNT[cell] as usize;
    let mut s = 0;
    let mut k = 0;
    while k < cn {
        let w = CW[cell * 20 + k] as usize;
        let b = q.wb[w] as usize;
        let o = q.ww[w] as usize;
        if b > 0 && o > 0 {
            k += 1;
            continue;
        }
        let cur = WSC[b * WSC_DIM + o];
        s += if side == BLACK { WSC[(b + 1) * WSC_DIM + o] - cur } else { WSC[b * WSC_DIM + o + 1] - cur };
        k += 1;
    }
    s
}

/** 合法落点写入 out(候选点里剔掉黑方禁手),返回个数 */
pub unsafe fn legal_moves(out: &mut [i32], side: u8, mode: i32) -> usize {
    let mut mb = [0i32; SIZE + 1];
    let mut ms = [0i32; SIZE + 1];
    let n = gen_candidates(side, &mut mb, &mut ms);
    let mut m = 0;
    let mut i = 0;
    while i < n {
        let cell = mb[i] as usize;
        if mode == RENJU && side == BLACK && is_forbidden(cell, 0) {
            i += 1;
            continue;
        }
        out[m] = cell as i32;
        m += 1;
        i += 1;
    }
    m
}

/* ==================== 评估 ==================== */

/** 静态评估(行棋方视角):线形分类 + 权重表(eval.rs),
 *  引擎唯一评估器。 */
pub unsafe fn evaluate(side: u8, mode: i32) -> i32 {
    eval::evaluate(side, mode)
}
