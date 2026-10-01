/* ============================================================
 * 静态评估:线形分类器 + 学习权重表(eval_tables.bin,CC0 数据)。
 *
 * 模型(行为规格,由权重文件格式与对拍夹具共同固定):
 *  - 每个空格在 4 个方向上各有一个「线形」:假设己方在此落子后,
 *    该方向 ±H 格窗口(无禁 H=4 / 有禁 H=5,出界视同对方子)所处
 *    的战术等级,共 16 类(类号即权重表索引序):
 *      0 死形  1 长连  2 眠一  3 活一  4 眠二  5 活二
 *      6 活二A 7 活二B 8 眠三  9 眠三X 10 活三 11 活三X
 *     12 眠四 13 眠四X 14 活四 15 成五
 *    有禁黑方额外规则:落子成 ≥6 连判长连类;同线双成五点(两点
 *    距离 <5)判长连类(即单线双四禁手)。
 *  - 四向线形排序后取字典序排名(0..3875)为该格该方的「格码」,
 *    查 EVALS 表得到该格黑方分;同时四向线形的多重集聚合出 14 类
 *    「威胁级」(有禁黑方另判双四/双三/长连为犯规级)。
 *  - 全盘空格黑方分求和 = 基础分;评估 = 相邻两手基础分的均值
 *    (先加后除,向零截断)+ 威胁表 THREAT[11 位掩码],钳 ±6000。
 *  - 掩码 11 位 = 双方威胁级存在性的按位打包(见 MASK_BITS)。
 *
 * 分类算法:窗口内每个空点递归「试放」再分类(每层多放一子,深度
 * 有界),窗口内容的三进制编码做记忆化;原地改写 + 回复,不复制。
 * 增量维护:落/悔子后只重算四方向 ±H 线内的空格;走子格的旧状态
 * 冻结(LIFO 撤销序下恰好恢复有效)。
 * ============================================================ */
use crate::*;

/* ---- 线形类号(权重表索引序,勿改) ---- */
pub const K_DEAD: u8 = 0;
pub const K_LONG: u8 = 1;
pub const K_SLEEP1: u8 = 2;
pub const K_LIVE1: u8 = 3;
pub const K_SLEEP2: u8 = 4;
pub const K_LIVE2: u8 = 5;
pub const K_LIVE2A: u8 = 6;
pub const K_LIVE2B: u8 = 7;
pub const K_SLEEP3: u8 = 8;
pub const K_SLEEP3X: u8 = 9;
pub const K_LIVE3: u8 = 10;
pub const K_LIVE3X: u8 = 11;
pub const K_SLEEP4: u8 = 12;
pub const K_SLEEP4X: u8 = 13;
pub const K_LIVE4: u8 = 14;
pub const K_FIVE: u8 = 15;
pub const KINDS: usize = 16;

/* ---- 威胁级类号(权重表掩码位定义用,勿改) ---- */
pub const A_NONE: usize = 0;
pub const A_FOUL: usize = 1;
pub const A_LIVE2: usize = 2;
pub const A_SLEEP3: usize = 3;
pub const A_LIVE2X2: usize = 4;
pub const A_SLEEP3P: usize = 5;
pub const A_LIVE3: usize = 6;
pub const A_LIVE3P: usize = 7;
pub const A_LIVE3X2: usize = 8;
pub const A_SLEEP4: usize = 9;
pub const A_SLEEP4P: usize = 10;
pub const A_SLEEP4L3: usize = 11;
pub const A_LIVE4: usize = 12;
pub const A_FIVE: usize = 13;
pub const AGGS: usize = 14;

const CODES: usize = 3876; // 不减四元组字典序排名数 C(19,4)
const MASKS: usize = 2048; // 11 位掩码
const WIN_MAX: i32 = 6000;
const MAXH: usize = 5; // 有禁半窗;无禁 4
const WLEN: usize = MAXH * 2 + 1;
const MEMO_KEYS: usize = 3usize.pow(WLEN as u32);

/* 半线索引:两侧各 H 格,按 2 位/格打包(近端在低位)。
 * 打包值 → 紧凑槽号(仅收录「墙后无子」的合法半线)。
 * 格位编码:0b00 墙 / 0b01 白 / 0b10 黑 / 0b11 空。 */
const SLOTS_F: usize = 121; // Σ 3^v, v=0..4
const SLOTS_R: usize = 364; // Σ 3^v, v=0..5

/* ==================== 分类器 ============================ */

/* 窗口格值:三进制 0 空 / 1 己 / 2 敌(与记忆化键一致) */
const GAP: u8 = 0;
const MINE: u8 = 1;
const FOE: u8 = 2;

/** 窗口 [c-H, c+H] 的三进制键(无前导;每次建表前清前缀,模式间不串)。
 *  越出整条线的位置记敌子。 */
fn window_key(line: &[u8], c: usize, h: usize) -> usize {
    let n = h * 2 + 1;
    let mut key = 0usize;
    for p in 0..n {
        let j = c as i64 - h as i64 + p as i64;
        let v = if j < 0 || j as usize >= line.len() { FOE } else { line[j as usize] };
        key = key * 3 + v as usize;
    }
    key
}

/** 以 c 为中心、假设己方在 c 落子后的线形。
 *  line 长度 2H+1,c 处已由调用方置为 MINE;递归试放会临时改写
 *  line 并在返回前恢复。foul = 有禁黑方视角。 */
fn kind_at(line: &mut [u8], c: usize, h: usize, foul: bool, memo: &mut [u8]) -> u8 {
    let key = window_key(line, c, h);
    if memo[key] != 0xff {
        return memo[key];
    }
    let n = line.len();
    let k: u8;

    /* 中心连长与可展开范围(遇敌子或线端为止) */
    let mut run = 1i32; // 含中心的连续己子数
    let mut lo = c;
    {
        let mut i = c;
        let mut counting = true;
        while i > 0 {
            i -= 1;
            match line[i] {
                MINE => {
                    if counting {
                        run += 1;
                    }
                }
                FOE => break,
                _ => counting = false,
            }
            lo = i;
        }
    }
    let mut hi = c;
    {
        let mut i = c;
        let mut counting = true;
        while i + 1 < n {
            i += 1;
            match line[i] {
                MINE => {
                    if counting {
                        run += 1;
                    }
                }
                FOE => break,
                _ => counting = false,
            }
            hi = i;
        }
    }

    if foul && run >= 6 {
        k = K_LONG;
    } else if run >= 5 {
        k = K_FIVE;
    } else if (hi - lo + 1) < 5 {
        k = K_DEAD;
    } else {
        /* 逐空点试放,统计子线形分布 */
        let mut tally = [0i32; KINDS];
        let mut five_at = [0usize; 2];
        let mut i = lo;
        while i <= hi {
            if line[i] == GAP {
                line[i] = MINE;
                let sub = kind_at(line, i, h, foul, memo);
                line[i] = GAP;
                if sub == K_FIVE && tally[K_FIVE as usize] < 2 {
                    five_at[tally[K_FIVE as usize] as usize] = i;
                }
                tally[sub as usize] += 1;
            }
            i += 1;
        }
        let t = |k: u8| tally[k as usize];
        let fives = t(K_FIVE);
        let threes = t(K_LIVE3) + t(K_LIVE3X);
        let twos = t(K_LIVE2) + t(K_LIVE2A) + t(K_LIVE2B);
        k = if fives >= 2 {
            if foul && five_at[1] - five_at[0] < 5 {
                K_LONG // 同线双四,黑方禁手
            } else {
                K_LIVE4
            }
        } else if fives == 1 {
            /* 唯一成五点被挡后线形仍 ≥ 眠三 → 连冲四 */
            line[five_at[0]] = FOE;
            let after_block = kind_at(line, c, h, foul, memo);
            line[five_at[0]] = GAP;
            if after_block >= K_SLEEP3 {
                K_SLEEP4X
            } else {
                K_SLEEP4
            }
        } else if t(K_LIVE4) >= 2 {
            K_LIVE3X
        } else if t(K_LIVE4) == 1 {
            K_LIVE3
        } else if t(K_SLEEP4X) >= 1 {
            K_SLEEP3X
        } else if t(K_SLEEP4) >= 1 {
            K_SLEEP3
        } else if threes >= 4 {
            K_LIVE2B
        } else if threes >= 3 {
            K_LIVE2A
        } else if threes >= 1 {
            K_LIVE2
        } else if t(K_SLEEP3) + t(K_SLEEP3X) >= 1 {
            K_SLEEP2
        } else if twos >= 1 {
            K_LIVE1
        } else if t(K_SLEEP2) >= 1 {
            K_SLEEP1
        } else {
            K_DEAD
        };
    }
    memo[key] = k;
    k
}

/* ==================== 表(全在 init 构建) ==================== */

/* 半线:打包值 → 槽号 */
static mut SLOT_F: [u16; 256] = [0; 256];
static mut SLOT_R: [u16; 1024] = [0; 1024];
/* (左槽,右槽) → 黑线形 | 白线形<<4 */
static mut LINE_F: [u8; SLOTS_F * SLOTS_F] = [0; SLOTS_F * SLOTS_F];
static mut LINE_R: [u8; SLOTS_R * SLOTS_R] = [0; SLOTS_R * SLOTS_R];
/* 四向线形 → 格码(低 12 位)| 威胁级(高 4 位);犯规版仅有禁黑方用 */
static mut CELL_PLAIN: [u16; KINDS * KINDS * KINDS * KINDS] = [0; KINDS * KINDS * KINDS * KINDS];
static mut CELL_FOUL: [u16; KINDS * KINDS * KINDS * KINDS] = [0; KINDS * KINDS * KINDS * KINDS];

/* 记忆化:犯规/普通各一份 */
static mut MEMO_FOUL: [u8; MEMO_KEYS] = [0; MEMO_KEYS];
static mut MEMO_PLAIN: [u8; MEMO_KEYS] = [0; MEMO_KEYS];

/* 权重(include 的 bin:头 12B + 6 张 i16 小端表) */
const TABLES: &[u8] = include_bytes!("eval_tables.bin");
static mut W_FREE: [i16; CODES] = [0; CODES];
static mut W_RBLACK: [i16; CODES] = [0; CODES];
static mut W_RWHITE: [i16; CODES] = [0; CODES];
static mut T_FREE: [i16; MASKS] = [0; MASKS];
static mut T_RBLACK: [i16; MASKS] = [0; MASKS];
static mut T_RWHITE: [i16; MASKS] = [0; MASKS];

static mut READY: bool = false;
static mut MODE: i32 = FREE;

/** 四向线形的多重集聚合 → 威胁级。foul = 有禁黑方。 */
fn aggregate(kinds: [u8; 4], foul: bool) -> usize {
    let mut m = [0i32; KINDS];
    for k in kinds {
        m[k as usize] += 1;
    }
    m[K_SLEEP4 as usize] += m[K_SLEEP4X as usize];
    m[K_SLEEP3 as usize] += m[K_SLEEP3X as usize];
    let g = |k: u8| m[k as usize];
    let threes = g(K_LIVE3) + g(K_LIVE3X);
    let twos = g(K_LIVE2) + g(K_LIVE2A) + g(K_LIVE2B);

    if g(K_FIVE) >= 1 {
        return A_FIVE;
    }
    if foul {
        if g(K_LONG) >= 1 {
            return A_FOUL;
        }
        if g(K_LIVE4) + g(K_SLEEP4) >= 2 {
            return A_FOUL;
        }
        if threes >= 2 {
            return A_FOUL;
        }
    }
    if g(K_SLEEP4) >= 2 || g(K_LIVE4) >= 1 {
        return A_LIVE4;
    }
    if g(K_SLEEP4) >= 1 {
        if threes >= 1 {
            return A_SLEEP4L3;
        }
        if g(K_SLEEP3) >= 1 || twos >= 1 {
            return A_SLEEP4P;
        }
        return A_SLEEP4;
    }
    if threes >= 1 {
        if threes >= 2 {
            return A_LIVE3X2;
        }
        if g(K_SLEEP3) >= 1 || twos >= 1 {
            return A_LIVE3P;
        }
        return A_LIVE3;
    }
    if g(K_SLEEP3) >= 2 || (g(K_SLEEP3) == 1 && twos >= 1) {
        return A_SLEEP3P;
    }
    if twos >= 2 {
        return A_LIVE2X2;
    }
    if g(K_SLEEP3) >= 1 {
        return A_SLEEP3;
    }
    if twos >= 1 {
        return A_LIVE2;
    }
    A_NONE
}

/** 格码表:非降四元组按字典序连号,任意排列继承排序代表。 */
unsafe fn build_cell_tables() {
    for (table, foul) in [
        (core::ptr::addr_of_mut!(CELL_PLAIN) as *mut u16, false),
        (core::ptr::addr_of_mut!(CELL_FOUL) as *mut u16, true),
    ] {
        let mut rank = 0u16;
        let mut a = 0usize;
        while a < KINDS {
            let mut b = a;
            while b < KINDS {
                let mut c = b;
                while c < KINDS {
                    let mut d = c;
                    while d < KINDS {
                        let agg = aggregate([a as u8, b as u8, c as u8, d as u8], foul) as u16;
                        *table.add(((a * KINDS + b) * KINDS + c) * KINDS + d) =
                            rank | (agg << 12);
                        rank += 1;
                        d += 1;
                    }
                    c += 1;
                }
                b += 1;
            }
            a += 1;
        }
        debug_assert_eq!(rank as usize, CODES);
        let mut idx = 0usize;
        while idx < KINDS * KINDS * KINDS * KINDS {
            let mut q = [
                idx & 15,
                (idx >> 4) & 15,
                (idx >> 8) & 15,
                (idx >> 12) & 15,
            ];
            q.sort_unstable();
            let src = ((q[0] * KINDS + q[1]) * KINDS + q[2]) * KINDS + q[3];
            if src != idx {
                *table.add(idx) = *table.add(src);
            }
            idx += 1;
        }
    }
}

/** 合法半线(墙后全是墙)的打包值集合 → 槽号表;返回槽数。 */
fn build_slots(h: usize, slots: &mut [u16]) -> usize {
    let span = 1usize << (h * 2);
    let mut next = 0usize;
    let mut packed = 0usize;
    while packed < span {
        let mut wall_seen = false;
        let mut ok = true;
        let mut k = 0;
        while k < h {
            let cell = (packed >> (k * 2)) & 3;
            if cell == 0 {
                wall_seen = true;
            } else if wall_seen {
                ok = false;
                break;
            }
            k += 1;
        }
        slots[packed] = if ok {
            let s = next;
            next += 1;
            s as u16
        } else {
            0
        };
        packed += 1;
    }
    next
}

/** 打包半线 → 三进制数字串(近端起,缺位补墙);返回实际位数。 */
fn unpack(packed: usize, h: usize, out: &mut [u8]) -> usize {
    let mut v = 0usize;
    let mut k = 0;
    while k < h {
        match (packed >> (k * 2)) & 3 {
            0 => break, // 墙起,其后视同墙
            cell => {
                out[v] = cell as u8;
                v += 1;
            }
        }
        k += 1;
    }
    v
}

/** 半线对表:两视角各分类一次,字节 = 黑 | 白<<4。 */
unsafe fn build_line_tables() {
    /* 无禁:H=4 */
    fill_line_table(
        4,
        &mut SLOT_F,
        core::ptr::addr_of_mut!(MEMO_PLAIN) as *mut u8,
        core::ptr::addr_of_mut!(MEMO_FOUL) as *mut u8,
        core::ptr::addr_of_mut!(LINE_F) as *mut u8,
    );
    /* 有禁:H=5 */
    fill_line_table(
        5,
        &mut SLOT_R,
        core::ptr::addr_of_mut!(MEMO_PLAIN) as *mut u8,
        core::ptr::addr_of_mut!(MEMO_FOUL) as *mut u8,
        core::ptr::addr_of_mut!(LINE_R) as *mut u8,
    );
}

unsafe fn fill_line_table(
    h: usize,
    slots: &mut [u16],
    memo_plain: *mut u8,
    memo_foul: *mut u8,
    out: *mut u8,
) {
    let nslots = build_slots(h, slots);
    let wlen = h * 2 + 1;
    let memo_len = 3usize.pow(wlen as u32);
    /* 记忆化按窗口长度复用前缀,每次建表前清哨兵 */
    for m in 0..memo_len {
        *memo_plain.add(m) = 0xff;
        *memo_foul.add(m) = 0xff;
    }
    let memo_plain =
        core::slice::from_raw_parts_mut(memo_plain, memo_len.max(MEMO_KEYS));
    let memo_foul =
        core::slice::from_raw_parts_mut(memo_foul, memo_len.max(MEMO_KEYS));

    /* 预解全部槽的数字串(近端起;槽号小 → 打包值小,顺序反查) */
    let mut digits = [[0u8; MAXH]; SLOTS_R];
    let mut dlen = [0usize; SLOTS_R];
    let mut s = 0usize;
    while s < nslots {
        dlen[s] = unpack(slot_packed(h, slots, s), h, &mut digits[s]);
        s += 1;
    }

    let mut line = [GAP; WLEN];
    let mid = h;
    let mut ls = 0usize;
    while ls < nslots {
        let mut rs = 0usize;
        while rs < nslots {
            let mut byte = 0u8;
            for black in [true, false] {
                line[mid] = MINE;
                /* 左右半线由近及远铺开;数字用尽处即墙,记敌子 */
                let mut i = 0usize;
                while i < h {
                    let d = if i < dlen[ls] { digits[ls][i] } else { 0 };
                    line[mid - 1 - i] = to_flag(d, black);
                    i += 1;
                }
                let mut j = 0usize;
                while j < h {
                    let d = if j < dlen[rs] { digits[rs][j] } else { 0 };
                    line[mid + 1 + j] = to_flag(d, black);
                    j += 1;
                }
                let foul = foul_table(h, black);
                let memo = if foul { &mut *memo_foul } else { &mut *memo_plain };
                let k = kind_at(&mut line[..wlen], mid, h, foul, memo);
                byte = if black { k } else { byte | (k << 4) };
            }
            *out.add(ls * nslots + rs) = byte;
            rs += 1;
        }
        ls += 1;
    }
}

/** 槽号反查打包值(build_slots 的伴随;槽数很小,线性扫可接受)。 */
fn slot_packed(h: usize, slots: &[u16], slot: usize) -> usize {
    let span = 1usize << (h * 2);
    let mut packed = 0usize;
    while packed < span {
        if slots[packed] as usize == slot && slot_valid(packed, h) {
            return packed;
        }
        packed += 1;
    }
    0
}

fn slot_valid(packed: usize, h: usize) -> bool {
    let mut wall_seen = false;
    let mut k = 0;
    while k < h {
        let cell = (packed >> (k * 2)) & 3;
        if cell == 0 {
            wall_seen = true;
        } else if wall_seen {
            return false;
        }
        k += 1;
    }
    true
}

/** 数字串值(1 白 / 2 黑 / 3 空;与 pair_bits 的 2 位编码一致)
 *  → 视角旗标;black 视角己方 = 黑。 */
fn to_flag(d: u8, black: bool) -> u8 {
    match d {
        3 => GAP,
        x if x == if black { 2 } else { 1 } => MINE,
        _ => FOE,
    }
}

/** 线形表的犯规开关:只有有禁(H=5)的黑视角需要犯规语义。 */
fn foul_table(h: usize, black: bool) -> bool {
    h == 5 && black
}

/* ==================== 初始化 ============================ */

pub unsafe fn ensure_init() {
    if READY {
        return;
    }
    /* 权重文件:魔数 "AHCE" + 版本 i32 + scale f32(已烘进表值,不另乘)
     * + 6 张 i16 小端表,顺序:无禁 / 有禁黑 / 有禁白 / 威胁×同三序 */
    let (mut ok, mut off) = (true, 12usize);
    ok &= TABLES[0] == b'A' && TABLES[1] == b'H' && TABLES[2] == b'C' && TABLES[3] == b'E';
    macro_rules! pull {
        ($dst:ident, $n:expr) => {{
            let d = core::ptr::addr_of_mut!($dst) as *mut i16;
            let mut i = 0;
            while i < $n {
                *d.add(i) = i16::from_le_bytes([TABLES[off + i * 2], TABLES[off + i * 2 + 1]]);
                i += 1;
            }
            off += $n * 2;
        }};
    }
    pull!(W_FREE, CODES);
    pull!(W_RBLACK, CODES);
    pull!(W_RWHITE, CODES);
    pull!(T_FREE, MASKS);
    pull!(T_RBLACK, MASKS);
    pull!(T_RWHITE, MASKS);
    debug_assert!(ok && off == TABLES.len());

    build_cell_tables();
    build_line_tables();
    READY = true;
}

pub unsafe fn set_mode(mode: i32) {
    MODE = mode;
}

#[inline]
unsafe fn half_len() -> usize {
    if MODE == RENJU {
        5
    } else {
        4
    }
}

/* ==================== 运行态(与 Pos 同生命周期) ==================== */

static mut DIR_KIND: [u8; SIZE * 4] = [0; SIZE * 4]; // 每格 4 向:黑线形 | 白<<4
static mut RANK_B: [u16; SIZE] = [0; SIZE];
static mut RANK_W: [u16; SIZE] = [0; SIZE];
static mut AGG_BW: [u8; SIZE] = [0; SIZE]; // 每格:黑威胁级 | 白<<4
static mut AGG_CNT: [[i32; AGGS]; 2] = [[0; AGGS]; 2];
static mut SUM: i32 = 0; // 空格黑方分总和
static mut SUM_HIST: [i32; SIZE + 2] = [0; SIZE + 2];

/** (r,c) 出界 → 0b00;空 0b11;黑 0b10;白 0b01。 */
#[inline]
unsafe fn pair_bits(r: i32, c: i32) -> u32 {
    if r < 0 || r >= N as i32 || c < 0 || c >= N as i32 {
        return 0;
    }
    match p().bd[(r * N as i32 + c as i32) as usize] {
        0 => 0b11,
        1 => 0b10,
        _ => 0b01,
    }
}

/** 沿方向 d 符号 s 的半线打包值(近端低位)。 */
#[inline]
unsafe fn pack_half(cell: usize, d: usize, sgn: i32, h: usize) -> u32 {
    let (r, c) = (cell / N, cell % N);
    let mut bits = 0u32;
    let mut k = 1usize;
    while k <= h {
        let (rr, cc) = (
            r as i32 + DR[d] * sgn * k as i32,
            c as i32 + DC[d] * sgn * k as i32,
        );
        bits |= pair_bits(rr, cc) << (2 * (k - 1));
        k += 1;
    }
    bits
}

/** 查半线对表:cell 方向 d 的(黑线形,白线形)。 */
#[inline]
unsafe fn line_kinds(cell: usize, d: usize, h: usize) -> u8 {
    let left = pack_half(cell, d, -1, h) as usize;
    let right = pack_half(cell, d, 1, h) as usize;
    if MODE == RENJU {
        let slot = core::ptr::addr_of!(SLOT_R) as *const u16;
        let table = core::ptr::addr_of!(LINE_R) as *const u8;
        *table.add(*slot.add(left) as usize * SLOTS_R + *slot.add(right) as usize)
    } else {
        let slot = core::ptr::addr_of!(SLOT_F) as *const u16;
        let table = core::ptr::addr_of!(LINE_F) as *const u8;
        *table.add(*slot.add(left) as usize * SLOTS_F + *slot.add(right) as usize)
    }
}

/** 四向线形 → (黑格码|威胁级<<12, 白格码|威胁级<<12)。 */
#[inline]
unsafe fn cell_entry(kinds: &[u8; 4]) -> [u16; 2] {
    let plain = core::ptr::addr_of!(CELL_PLAIN) as *const u16;
    let foul = core::ptr::addr_of!(CELL_FOUL) as *const u16;
    let idx = |black: bool| -> usize {
        let mut v = 0usize;
        for d in 0..4 {
            let k = if black { kinds[d] & 0xf } else { kinds[d] >> 4 } as usize;
            v = v * KINDS + k;
        }
        v
    };
    let base_b = if MODE == RENJU { foul } else { plain };
    [*base_b.add(idx(true)), *plain.add(idx(false))]
}

/** 空格黑方分:己方格码分 − 对方格码分(按模式选表)。 */
#[inline]
unsafe fn cell_score(rb: u16, rw: u16) -> i32 {
    if MODE == RENJU {
        let (eb, ew) = (
            core::ptr::addr_of!(W_RBLACK) as *const i16,
            core::ptr::addr_of!(W_RWHITE) as *const i16,
        );
        *eb.add(rb as usize) as i32 - *ew.add(rw as usize) as i32
    } else {
        let e = core::ptr::addr_of!(W_FREE) as *const i16;
        *e.add(rb as usize) as i32 - *e.add(rw as usize) as i32
    }
}

/* ==================== 状态维护 ============================ */

/** 单格四向线形 + 格码全量重算(仅空格调用;占用格冻结旧值)。 */
unsafe fn refresh_cell(cell: usize, h: usize) {
    let mut kinds = [0u8; 4];
    for d in 0..4 {
        kinds[d] = line_kinds(cell, d, h);
        DIR_KIND[cell * 4 + d] = kinds[d];
    }
    let e = cell_entry(&kinds);
    RANK_B[cell] = e[0] & 0xfff;
    RANK_W[cell] = e[1] & 0xfff;
    AGG_BW[cell] = ((e[0] >> 12) as u8) | (((e[1] >> 12) as u8) << 4);
}

/** 空盘全量(bd 已清空)。 */
pub unsafe fn reset() {
    ensure_init();
    let h = half_len();
    AGG_CNT = [[0; AGGS]; 2];
    SUM = 0;
    let mut cell = 0usize;
    while cell < SIZE {
        refresh_cell(cell, h);
        SUM += cell_score(RANK_B[cell], RANK_W[cell]);
        AGG_CNT[0][(AGG_BW[cell] & 0xf) as usize] += 1;
        AGG_CNT[1][(AGG_BW[cell] >> 4) as usize] += 1;
        cell += 1;
    }
    SUM_HIST[0] = SUM;
}

/** 目标空格单方向线形变化后的差分。
 *  kinds 里 d 位已更新为新值,其余方向取现值。 */
unsafe fn apply_dir_change(t: usize, d: usize, new: u8) {
    let mut kinds = [0u8; 4];
    for i in 0..4 {
        kinds[i] = if i == d { new } else { DIR_KIND[t * 4 + i] };
    }
    let e = cell_entry(&kinds);
    let (rb, rw) = (e[0] & 0xfff, e[1] & 0xfff);
    let (ab, aw) = ((e[0] >> 12) as usize, (e[1] >> 12) as usize);

    SUM += cell_score(rb, rw) - cell_score(RANK_B[t], RANK_W[t]);
    AGG_CNT[0][(AGG_BW[t] & 0xf) as usize] -= 1;
    AGG_CNT[1][(AGG_BW[t] >> 4) as usize] -= 1;
    AGG_CNT[0][ab] += 1;
    AGG_CNT[1][aw] += 1;

    DIR_KIND[t * 4 + d] = new;
    RANK_B[t] = rb;
    RANK_W[t] = rw;
    AGG_BW[t] = (ab as u8) | ((aw as u8) << 4);
}

/** 落子/提子后(bd 已更新)维护:重算四向 ±H 线内空格,
 *  再按方向增减走子格冻结的自身贡献。added = 落子。 */
pub unsafe fn after_stone_change(cell: usize, added: bool) {
    let h = half_len();
    let (r, c) = (cell / N, cell % N);
    for d in 0..4 {
        let mut k = 1i32;
        while k <= h as i32 {
            for sgn in [-1i32, 1] {
                let (rr, cc) = (r as i32 + DR[d] * sgn * k, c as i32 + DC[d] * sgn * k);
                if rr >= 0 && rr < N as i32 && cc >= 0 && cc < N as i32 {
                    let t = (rr * N as i32 + cc as i32) as usize;
                    if p().bd[t] == 0 {
                        let fresh = line_kinds(t, d, h);
                        if fresh != DIR_KIND[t * 4 + d] {
                            apply_dir_change(t, d, fresh);
                        }
                    }
                }
            }
            k += 1;
        }
    }
    /* 走子格:占用期间不再计值;提子后凭冻结值原样加回 */
    let score = cell_score(RANK_B[cell], RANK_W[cell]);
    let ab = (AGG_BW[cell] & 0xf) as usize;
    let aw = (AGG_BW[cell] >> 4) as usize;
    if added {
        SUM -= score;
        AGG_CNT[0][ab] -= 1;
        AGG_CNT[1][aw] -= 1;
    } else {
        SUM += score;
        AGG_CNT[0][ab] += 1;
        AGG_CNT[1][aw] += 1;
    }
}

/** make 之后调用:记录该手之后的快照。 */
pub unsafe fn snapshot() {
    SUM_HIST[p().stones as usize] = SUM;
}

/* ==================== 评估 ============================ */

/* 掩码位定义:bit 号,自己/对方,命中任一威胁级即置位 */
const MASK_BITS: [(u16, bool, [usize; 2]); 11] = [
    (0, false, [A_FIVE, AGGS]),
    (1, true, [A_LIVE4, AGGS]),
    (2, false, [A_LIVE4, AGGS]),
    (3, true, [A_SLEEP4P, A_SLEEP4L3]),
    (4, true, [A_SLEEP4, AGGS]),
    (5, true, [A_LIVE3P, A_LIVE3X2]),
    (6, true, [A_LIVE3, AGGS]),
    (7, false, [A_SLEEP4P, A_SLEEP4L3]),
    (8, false, [A_SLEEP4, AGGS]),
    (9, false, [A_LIVE3P, A_LIVE3X2]),
    (10, false, [A_LIVE3, AGGS]),
];

/** 双方威胁级存在性 → 11 位掩码(black = 行棋方是黑)。 */
unsafe fn threat_mask(black: bool) -> usize {
    let cnt = core::ptr::addr_of!(AGG_CNT) as *const [i32; AGGS];
    let mut mask = 0usize;
    for (bit, mine, classes) in MASK_BITS {
        let side = if mine == black { 0 } else { 1 };
        let hit = unsafe {
            (*cnt.add(side))[classes[0]] > 0
                || (classes[1] < AGGS && (*cnt.add(side))[classes[1]] > 0)
        };
        if hit {
            mask |= 1 << bit;
        }
    }
    mask
}

/** 静态评估(行棋方视角):两拍均值 + 威胁项,钳 ±6000。 */
pub unsafe fn evaluate(side: u8, mode: i32) -> i32 {
    ensure_init();
    let stones = p().stones as usize;
    let cur = SUM_HIST[stones];
    let prev = if stones >= 1 { SUM_HIST[stones - 1] } else { SUM_HIST[0] };
    let (x, y) = if side == BLACK { (cur, prev) } else { (-cur, -prev) };
    let base = (x + y) / 2; // 先求和再除:向零截断,与真值口径一致

    let black = side == BLACK;
    let mask = threat_mask(black);
    let threat = if mode == RENJU {
        let t = if black {
            core::ptr::addr_of!(T_RBLACK) as *const i16
        } else {
            core::ptr::addr_of!(T_RWHITE) as *const i16
        };
        *t.add(mask) as i32
    } else {
        let t = core::ptr::addr_of!(T_FREE) as *const i16;
        *t.add(mask) as i32
    };
    (base + threat).clamp(-WIN_MAX, WIN_MAX)
}

/* ==================== 工具(测试 / 调参) ==================== */

/** 从 bd 全量重建(空格重算,占用格保留冻结值)。 */
pub unsafe fn rebuild_from_board() {
    ensure_init();
    let h = half_len();
    AGG_CNT = [[0; AGGS]; 2];
    SUM = 0;
    let q = p();
    let mut cell = 0usize;
    while cell < SIZE {
        if q.bd[cell] == 0 {
            refresh_cell(cell, h);
            SUM += cell_score(RANK_B[cell], RANK_W[cell]);
            AGG_CNT[0][(AGG_BW[cell] & 0xf) as usize] += 1;
            AGG_CNT[1][(AGG_BW[cell] >> 4) as usize] += 1;
        }
        cell += 1;
    }
    SUM_HIST[q.stones as usize] = SUM;
}

/** 调参特征导出:空格格码直方图(黑/白)+ 行棋方威胁掩码。 */
pub unsafe fn feature_dump(hist_b: &mut [i16; 3876], hist_w: &mut [i16; 3876], mask: &mut usize) {
    ensure_init();
    let q = p();
    for v in hist_b.iter_mut() {
        *v = 0;
    }
    for v in hist_w.iter_mut() {
        *v = 0;
    }
    let mut cell = 0usize;
    while cell < SIZE {
        if q.bd[cell] == 0 {
            hist_b[RANK_B[cell] as usize] += 1;
            hist_w[RANK_W[cell] as usize] += 1;
        }
        cell += 1;
    }
    *mask = threat_mask(side_to_move() == BLACK);
}

/** 当前基础分(调参导出用)。 */
pub unsafe fn sum_dump() -> i32 {
    SUM
}

/** 外源权重装载(调参后验证;测试用)。 */
pub unsafe fn install_weights(
    evals_f: &[i16],
    evals_rb: &[i16],
    evals_rw: &[i16],
    threat_f: &[i16],
    threat_rb: &[i16],
    threat_rw: &[i16],
) {
    ensure_init();
    let dsts = [
        core::ptr::addr_of_mut!(W_FREE) as *mut i16,
        core::ptr::addr_of_mut!(W_RBLACK) as *mut i16,
        core::ptr::addr_of_mut!(W_RWHITE) as *mut i16,
        core::ptr::addr_of_mut!(T_FREE) as *mut i16,
        core::ptr::addr_of_mut!(T_RBLACK) as *mut i16,
        core::ptr::addr_of_mut!(T_RWHITE) as *mut i16,
    ];
    let srcs: [&[i16]; 6] = [evals_f, evals_rb, evals_rw, threat_f, threat_rb, threat_rw];
    for (dst, src) in dsts.iter().zip(srcs.iter()) {
        for i in 0..src.len() {
            *dst.add(i) = src[i];
        }
    }
}

/** 内部态导出(增量 vs 全量对拍用)。 */
/** 调试:逐格打印 4 向线形(对拍用;原生测试侧专用)。 */
#[cfg(not(target_arch = "wasm32"))]
pub unsafe fn debug_dump_cells() {
    ensure_init();
    let mut cell = 0usize;
    while cell < SIZE {
        let (x, y) = (cell % N, cell / N);
        print!(" {},{}", x, y);
        for d in 0..4 {
            print!(" {:x}{:x}", DIR_KIND[cell * 4 + d] & 0xf, DIR_KIND[cell * 4 + d] >> 4);
        }
        cell += 1;
    }
    println!();
}
pub unsafe fn export_state(sum: &mut i32, agg: &mut [[i32; AGGS]; 2]) {
    *sum = SUM;
    *agg = AGG_CNT;
}
