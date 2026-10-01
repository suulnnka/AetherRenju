/* ============================================================
 * VCT 根证明搜索(AND-OR 双层证明):
 *   OR(攻方):成五点 → 冲四 → 强三(一步造 ≥2 个三)。
 *   AND(守方):守方有成五点 → 攻败;攻方 ≥2 个成五点 → 攻胜;
 *     防着 = 堵点 / 争夺攻方冲四点 ∪ 守方反击四(漏掉反击四
 *     会产生胜误判)。
 * 置换表只复用负结论(正证明复用可能掩盖未验证分支,RQ550);
 * 三攻击防守集是启发式收窄。
 * 连珠:黑方着法全过 is_forbidden;成五点剔除长连补点。
 * 超限一律按「无胜」返回。
 * ============================================================ */
use crate::*;
use crate::search::{s, tier_of, time_up};

const VCT_TT_BITS: usize = 13;
const VCT_TT_SIZE: usize = 1 << VCT_TT_BITS;
const VCT_TT_MASK: u64 = (VCT_TT_SIZE - 1) as u64;

/* 负结论 TT:键 = 盘面 Zobrist(OR 节点攻方固定,子数奇偶即轮次)。
 * VT_D = 0 表示空;命中且深度 ≥ 当前 → 攻方在此已证失败。 */
static mut VT_K: [u64; VCT_TT_SIZE] = [0; VCT_TT_SIZE];
static mut VT_D: [i16; VCT_TT_SIZE] = [0; VCT_TT_SIZE];

unsafe fn vct_tt_probe(key: u64, depth: i32) -> bool {
    let i = (key & VCT_TT_MASK) as usize;
    VT_K[i] == key && VT_D[i] as i32 >= depth
}
unsafe fn vct_tt_store(key: u64, depth: i32) {
    let i = (key & VCT_TT_MASK) as usize;
    if (VT_D[i] as i32) < depth {
        VT_K[i] = key;
        VT_D[i] = depth as i16;
    }
}

/** 走法合法性:有禁模式黑方要过禁手判定 */
#[inline]
unsafe fn playable(cell: usize, side: u8, mode: i32) -> bool {
    !(mode == RENJU && side == BLACK && is_forbidden(cell, 0))
}

/** cell 是否攻方的「成五点」(落入攻方纯 4 窗)。
 *  有禁黑方额外排除长连补点(补了成六连,是禁手不是五)。 */
unsafe fn is_five_point(cell: usize, side: u8, mode: i32) -> bool {
    if tier_of(cell, side) != 7 {
        return false;
    }
    if mode == RENJU && side == BLACK {
        /* 恰好五才算;长连补点对黑不是胜利点 */
        make(cell, BLACK);
        let ok = made_five(cell, RENJU);
        unmake(cell, BLACK);
        return ok;
    }
    true
}

/** 收集「攻方成五点」;返回个数(去重即天然:每 cell 一次判定) */
unsafe fn five_points(side: u8, mode: i32, out: &mut [usize]) -> usize {
    let q = p();
    let mut n = 0usize;
    let mut i = 0;
    while i < SIZE {
        if q.bd[i] == 0 && q.nei[i] > 0 && is_five_point(i, side, mode) {
            out[n] = i;
            n += 1;
        }
        i += 1;
    }
    n
}

/** 收集攻方「冲四点」(落入攻方纯 3 窗,含双四):走后出现 ≥1 成五威胁 */
unsafe fn four_moves(side: u8, mode: i32, out: &mut [usize]) -> usize {
    let q = p();
    let mut n = 0usize;
    let mut i = 0;
    while i < SIZE {
        if q.bd[i] == 0 && q.nei[i] > 0 {
            let t = tier_of(i, side);
            if (t == 3 || t == 5) && playable(i, side, mode) {
                out[n] = i;
                n += 1;
            }
        }
        i += 1;
    }
    n
}

/** 收集「强三点」:一步同时造 ≥2 个攻方纯 3 窗(双三/活三形态) */
unsafe fn strong_threes(side: u8, mode: i32, out: &mut [usize]) -> usize {
    let q = p();
    let mut n = 0usize;
    let mut i = 0;
    while i < SIZE {
        if q.bd[i] == 0 && q.nei[i] > 0 {
            let cn = CWCOUNT[i] as usize;
            let mut m2 = 0i32;
            let mut k = 0;
            while k < cn {
                let w = CW[i * 20 + k] as usize;
                let b = q.wb[w] as usize;
                let o = q.ww[w] as usize;
                let (m, oo) = if side == BLACK { (b, o) } else { (o, b) };
                if oo == 0 && m == 2 {
                    m2 += 1;
                }
                k += 1;
            }
            if m2 >= 2 && playable(i, side, mode) {
                out[n] = i;
                n += 1;
            }
        }
        i += 1;
    }
    n
}

/** OR 节点(攻方走):返回证明获胜的首着;失败/超限返回 None。 */
unsafe fn vct_or(attacker: u8, mode: i32, depth: i32, or_level: u32) -> Option<i32> {
    (*s()).nodes += 1;
    if depth <= 0 || time_up() {
        return None;
    }
    let key = (*p()).hk;
    if vct_tt_probe(key, depth) {
        return None;
    }

    let mut buf = [0usize; SIZE + 1];

    /* 1. 成五点:立即赢 */
    let n5 = five_points(attacker, mode, &mut buf);
    let mut i = 0;
    while i < n5 {
        let cell = buf[i];
        if playable(cell, attacker, mode) {
            make(cell, attacker);
            let win = made_five(cell, mode);
            unmake(cell, attacker);
            if win {
                return Some(cell as i32);
            }
        }
        i += 1;
    }

    /* 2. 冲四:造单成五威胁,守方被迫应对 */
    let n4 = four_moves(attacker, mode, &mut buf);
    let mut i = 0;
    while i < n4 {
        let cell = buf[i];
        make(cell, attacker);
        let win = made_five(cell, mode) || vct_and(attacker, mode, depth - 1, or_level);
        unmake(cell, attacker);
        if win {
            return Some(cell as i32);
        }
        if (*s()).aborted {
            return None;
        }
        i += 1;
    }

    /* 3. 强三(双三/活三类):更宽的攻击词汇,深一层才展开 */
    if or_level < 4 {
        let n3 = strong_threes(attacker, mode, &mut buf);
        let mut i = 0;
        while i < n3 {
            let cell = buf[i];
            make(cell, attacker);
            let win = made_five(cell, mode) || vct_and(attacker, mode, depth - 1, or_level);
            unmake(cell, attacker);
            if win {
                return Some(cell as i32);
            }
            if (*s()).aborted {
                return None;
            }
            i += 1;
        }
    }

    vct_tt_store(key, depth);
    None
}

/** AND 节点(守方走):所有防着都无法阻止攻方 → true。 */
unsafe fn vct_and(attacker: u8, mode: i32, depth: i32, or_level: u32) -> bool {
    (*s()).nodes += 1;
    if depth <= 0 || time_up() {
        return false;
    }
    let defender = attacker ^ 1;

    /* 守方自己有成五点 → 攻方路线失败 */
    let mut buf = [0usize; SIZE + 1];
    if five_points(defender, mode, &mut buf) > 0 {
        return false;
    }

    /* 攻方成五点集合 S */
    let ns = five_points(attacker, mode, &mut buf);
    if ns >= 2 {
        return true; // 堵不过来(堵一处,另一处补五即胜)
    }

    /* 防着集:
     *   ns == 1(攻方刚冲四):唯一活路是堵 S[0];反击四来不及
     *     (攻方下一手直接补五),无需枚举。
     *   ns == 0(攻方刚做强三):守方要争夺攻方冲四点,或自己
     *     反击四强迫攻方应答;远处纯防守不枚举。 */
    let mut defs = [0usize; SIZE + 1];
    let ndef;
    if ns == 1 {
        defs[0] = buf[0];
        ndef = 1;
    } else {
        let n_atk = four_moves(attacker, mode, &mut buf); // 借 buf 再用
        let mut m = 0usize;
        let mut i = 0;
        while i < n_atk {
            defs[m] = buf[i];
            m += 1;
            i += 1;
        }
        let n_cnt = four_moves(defender, mode, &mut buf);
        let mut i = 0;
        while i < n_cnt {
            if m < SIZE {
                defs[m] = buf[i];
                m += 1;
            }
            i += 1;
        }
        ndef = m;
    }

    let mut i = 0;
    while i < ndef {
        let d = defs[i];
        if !playable(d, defender, mode) {
            i += 1;
            continue;
        }
        make(d, defender);
        let win = made_five(d, mode) || vct_or(attacker, mode, depth - 1, or_level + 1).is_some();
        unmake(d, defender);
        if !win {
            return false; // 守方这一防活下来了
        }
        if (*s()).aborted {
            return false;
        }
        i += 1;
    }
    /* 防着全不可下 = 守方无路 → 攻方胜 */
    true
}

/** 根入口:证明「当前行棋方存在强制连击胜」则返回首着。 */
pub unsafe fn search(attacker: u8, mode: i32, max_depth: i32) -> Option<i32> {
    /* 快速排除:一点威胁都没有的局面不进树 */
    let mut buf = [0usize; SIZE + 1];
    if five_points(attacker, mode, &mut buf) == 0
        && four_moves(attacker, mode, &mut buf) == 0
        && strong_threes(attacker, mode, &mut buf) == 0
    {
        return None;
    }
    vct_or(attacker, mode, max_depth, 0)
}
