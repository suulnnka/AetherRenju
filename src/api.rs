/* ============================================================
 * WASM / C ABI(不用 wasm-bindgen,体积最小)。全静态缓冲,
 * 线性内存不增长,JS 的 TypedArray 视图稳定。
 * 输入:ar_in_ptr() 拿指针,写落点序列(u8, 0..224)。
 * 输出(i32):
 *   LEVELS_OUT  4 档 × (depth, nodes, ms, jitter),引擎自报难度
 *   STATE_OUT   [0]stm [1]over [2]winner [3]reason [4]lineLen
 *              [5..14]连线 [15..240)棋盘 [240]禁手数 [241..]禁手
 *   SEARCH_OUT  [0]move [1]score [2]depth [3]nodes [4]flags
 *              (bit0 mate/bit1 only/bit2 draw/bit3 开局策略着法)
 *              [5]rootLen [6..]对
 * 开局库:ar_opening(ptr,len) → 1..=26/0(名字查询);
 *   ar_opening_name_ptr/len → 名字(线性内存静态串,零拷贝);
 *   ar_book_enable(0/1) → 开局策略开关(默认开)
 * ============================================================ */
use crate::*;

pub const STATE_LEN: usize = 466;
pub const SEARCH_LEN: usize = 6 + 2 * (SIZE + 1);
pub const LEVELS_LEN: usize = 16;

pub static mut IN: [u8; SIZE + 32] = [0; SIZE + 32];
pub static mut STATE_OUT: [i32; STATE_LEN] = [0; STATE_LEN];
pub static mut SEARCH_OUT: [i32; SEARCH_LEN] = [0; SEARCH_LEN];
pub static mut LEVELS_OUT: [i32; LEVELS_LEN] = [
    2, 8000, 400, 60, // 初级:2 层 + 随机化
    4, 40000, 1200, 0, // 中级
    6, 200000, 2500, 0, // 高级
    8, 600000, 5000, 0, // 大师
];

/* 原因码(worker.js 侧翻译成字符串) */
const REASON_FIVE: i32 = 1;
const REASON_OVERLINE: i32 = 2;
const REASON_FULL: i32 = 3;
const REASON_NO_LEGAL: i32 = 4;

/* SEARCH_OUT[4] 的 flags:bit3 = 开局策略着法(无搜索) */
const FLAG_BOOK: i32 = 8;

/// 开局策略开关(默认开;基准 / 测试关闭,让前三手也走搜索)
static mut BOOK_ON: u32 = 1;

/** 重演序列并产出 UI 渲染所需的全部规则事实(worker 的 state 契约)。
 *  返回 0 = 正常(写 STATE_OUT);-1 = 序列非法。 */
pub unsafe fn state(moves: &[u8], mode: i32) -> i32 {
    new_board();
    let replayed = replay_moves(moves, mode);
    if replayed == -1 {
        return -1;
    }
    let out = &mut *core::ptr::addr_of_mut!(STATE_OUT);
    for v in out.iter_mut() {
        *v = 0;
    }
    let last = if !moves.is_empty() {
        moves[moves.len() - 1] as usize
    } else {
        usize::MAX
    };

    let mut line = [0usize; 15];
    if replayed == -2 {
        /* 末手成五(有效终局):胜者 = 刚落子的一方(黑先,偶下标黑) */
        let winner = (moves.len() - 1) % 2;
        let llen = check_win(last, mode, &mut line);
        out[0] = (winner ^ 1) as i32; // 终局后轮次仅供 UI 自洽
        out[1] = 1;
        out[2] = winner as i32;
        out[3] = if llen > 5 { REASON_OVERLINE } else { REASON_FIVE };
        out[4] = llen as i32;
        let mut i = 0;
        while i < llen && i < 9 {
            out[5 + i] = line[i] as i32;
            i += 1;
        }
    } else {
        let stm = replayed as u8;
        out[0] = stm as i32;
        let mut over = 0;
        let mut winner = -1;
        let mut reason = 0;
        let mut llen = 0usize;
        let l = if last != usize::MAX { check_win(last, mode, &mut line) } else { 0 };
        if l > 0 {
            over = 1;
            winner = (1 - stm as i32) as i32;
            llen = l;
            reason = if l > 5 { REASON_OVERLINE } else { REASON_FIVE };
        } else if moves.len() >= SIZE {
            over = 1;
            reason = REASON_FULL;
        } else if mode == RENJU && stm == BLACK && {
            let mut cells = [0i32; SIZE + 1];
            legal_moves(&mut cells, BLACK, RENJU) == 0
        } {
            over = 1;
            winner = WHITE as i32;
            reason = REASON_NO_LEGAL;
        }
        out[1] = over;
        out[2] = winner;
        out[3] = reason;
        out[4] = llen as i32;
        let mut i = 0;
        while i < llen && i < 9 {
            out[5 + i] = line[i] as i32;
            i += 1;
        }
        /* 禁手点:只在「未终局 + 有禁 + 轮黑」时计算(UI 画 × 用) */
        if over == 0 && mode == RENJU && stm == BLACK {
            let mut fp = [0usize; SIZE];
            let n = forbidden_points(&mut fp);
            out[240] = n as i32;
            let mut i = 0;
            while i < n {
                out[241 + i] = fp[i] as i32;
                i += 1;
            }
        }
    }
    /* 棋盘 */
    let q = p();
    let mut i = 0;
    while i < SIZE {
        out[15 + i] = q.bd[i] as i32;
        i += 1;
    }
    0
}

/** 搜索当前局面(moves 重演后行棋方执子);结果写 SEARCH_OUT。
 *  返回 0 = 正常;-1 = 序列非法 / 已终局。
 *  开局策略(trivial bestmove):前三手且位形在 26 开局
 *  域内 → 直接给点,不走搜索(depth/nodes = 0,flags bit3 = book);
 *  ar_book_enable(0) 可关闭(基准 / 测试用)。 */
pub unsafe fn search(moves: &[u8], mode: i32, depth: i32, nodes: u64, ms: u64) -> i32 {
    new_board();
    let side = replay_moves(moves, mode);
    if side < 0 {
        return -1;
    }
    let out = &mut *core::ptr::addr_of_mut!(SEARCH_OUT);
    for v in out.iter_mut() {
        *v = 0;
    }
    if *core::ptr::addr_of!(BOOK_ON) != 0 {
        if let Some(mv) = opening::policy_move(moves) {
            out[0] = mv as i32;
            out[4] = FLAG_BOOK;
            return 0;
        }
    }
    search::search_best(mode, depth, nodes, ms);
    let st = &*core::ptr::addr_of_mut!(crate::search::S);
    out[0] = st.best;
    out[1] = st.best_score;
    out[2] = st.done_depth;
    out[3] = st.nodes as i32;
    let mut flags = 0;
    if st.mate {
        flags |= 1;
    }
    if st.only {
        flags |= 2;
    }
    if st.draw {
        flags |= 4;
    }
    out[4] = flags;
    out[5] = st.root_len as i32;
    let mut i = 0;
    while i < st.root_len {
        out[6 + 2 * i] = st.root_moves[i];
        out[7 + 2 * i] = st.root_scores[i];
        i += 1;
    }
    0
}

/* ==================== 导出(#[no_mangle] 保名) ==================== */

#[no_mangle]
pub extern "C" fn ar_version() -> u32 {
    2 // 引擎 ABI 版本(worker.js 校验)
}

#[no_mangle]
pub extern "C" fn ar_in_ptr() -> *mut u8 {
    core::ptr::addr_of_mut!(IN) as *mut u8
}

#[no_mangle]
pub extern "C" fn ar_levels_ptr() -> *const i32 {
    core::ptr::addr_of!(LEVELS_OUT) as *const i32
}

#[no_mangle]
pub extern "C" fn ar_levels_len() -> u32 {
    LEVELS_LEN as u32
}

#[no_mangle]
pub extern "C" fn ar_state_ptr() -> *const i32 {
    core::ptr::addr_of!(STATE_OUT) as *const i32
}

#[no_mangle]
pub extern "C" fn ar_state_len() -> u32 {
    STATE_LEN as u32
}

#[no_mangle]
pub extern "C" fn ar_search_ptr() -> *const i32 {
    core::ptr::addr_of!(SEARCH_OUT) as *const i32
}

#[no_mangle]
pub extern "C" fn ar_search_len() -> u32 {
    SEARCH_LEN as u32
}

/// moves 指针 = ar_in_ptr() 返回值;len ≤ 225
#[no_mangle]
pub unsafe extern "C" fn ar_state(moves: *const u8, len: u32, mode: u32) -> i32 {
    if len as usize > SIZE {
        return -1;
    }
    let mv = core::slice::from_raw_parts(moves, len as usize);
    state(mv, mode as i32)
}

#[no_mangle]
pub unsafe extern "C" fn ar_search(
    moves: *const u8,
    len: u32,
    mode: u32,
    depth: u32,
    nodes: u32,
    ms: u32,
) -> i32 {
    if len as usize > SIZE || depth == 0 || depth > 64 {
        return -1;
    }
    let mv = core::slice::from_raw_parts(moves, len as usize);
    search(mv, mode as i32, depth as i32, nodes as u64, ms as u64)
}

/// 开局策略开关:on = 1 开(默认)/ 0 关(搜索路径,基准用)
#[no_mangle]
pub unsafe extern "C" fn ar_book_enable(on: u32) {
    *core::ptr::addr_of_mut!(BOOK_ON) = if on == 0 { 0 } else { 1 };
}

/* ==================== 开局库(名字查询,协议参考 AetherOthello)====================
 * 26 开局的形状匹配与名字都住在引擎里;ar_opening 查询上次写进 IN 的
 * 序列(与 ar_state 同一份输入缓冲),命中后经 ar_opening_name_ptr/len
 * 零拷贝读名字(指针直指线性内存里的静态串)。名字只进 state 回包供
 * UI 显示;走子侧只有前三手的开局策略(search 里的 policy_move 路径,
 * trivial bestmove),搜索阶段不读开局库。 */

/// 上一次 ar_opening 的命中(0 = 无;1..=26 = 直指 1..13、斜指 14..26)
static mut LAST_OPENING: u32 = 0;

/// 前三手命中 26 开局?返回 1..=26(后续 name 导出用),0 = 未命中。
/// moves 指针 = ar_in_ptr() 返回值;len ≤ 225,只看前三手。
#[no_mangle]
pub unsafe extern "C" fn ar_opening(moves: *const u8, len: u32) -> u32 {
    let r = if len as usize > SIZE {
        None
    } else {
        opening::lookup(core::slice::from_raw_parts(moves, len as usize))
    };
    let v = r.map_or(0, |(kind, i)| (kind * 13 + i + 1) as u32);
    *core::ptr::addr_of_mut!(LAST_OPENING) = v;
    v
}

/// 命中开局的 UTF-8 名字(直指线性内存静态串);未命中返回 0。
#[no_mangle]
pub unsafe extern "C" fn ar_opening_name_ptr() -> u32 {
    let v = *core::ptr::addr_of!(LAST_OPENING);
    if v == 0 || v > 26 {
        return 0;
    }
    let (kind, i) = (((v - 1) / 13) as usize, ((v - 1) % 13) as usize);
    opening::name(kind, i).as_ptr() as u32
}

/// 名字字节长(与 ptr 配对;未命中 = 0)
#[no_mangle]
pub unsafe extern "C" fn ar_opening_name_len() -> u32 {
    let v = *core::ptr::addr_of!(LAST_OPENING);
    if v == 0 || v > 26 {
        return 0;
    }
    let (kind, i) = (((v - 1) / 13) as usize, ((v - 1) % 13) as usize);
    opening::name(kind, i).len() as u32
}
