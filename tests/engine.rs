/* 五子棋 / 连珠引擎测试:胜负判定 + 禁手用例 + 搜索行为 + 随机对局。
 *
 * 禁手判定的金标准是 brute 模块里的**独立暴力判定器**:按 RIF 定义
 * 逐点枚举、零增量、零共享缓冲,与引擎的增量实现完全不同路,随机
 * 局面上必须逐点一致(移植自旧 test/engine-test.mjs)。
 *
 * 运行:cargo test --release
 */
use aether_renju::*;
use aether_renju::search::search_best;


/* ---------- 全局测试锁:引擎状态是单一全局盘面,测试必须串行 ---------- */
use std::sync::{Mutex, MutexGuard, OnceLock};
static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn lock() -> MutexGuard<'static, ()> {
    TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

/* ---------- 摆棋助手:'.' 空 / 'x' 黑 / 'o' 白 / '*' 待测空点 ---------- */
fn pos(spec: &[(usize, &str)]) -> ([u8; SIZE], Option<usize>) {
    let mut bd = [0u8; SIZE];
    let mut star = None;
    for &(r, row) in spec {
        let cs: Vec<char> = row.chars().collect();
        for c in 0..15 {
            let ch = *cs.get(c).unwrap_or(&'.');
            match ch {
                'x' => bd[r * 15 + c] = 1,
                'o' => bd[r * 15 + c] = 2,
                '*' => star = Some(r * 15 + c),
                _ => {}
            }
        }
    }
    unsafe { sync_position(&bd) };
    (bd, star)
}
const fn cell(r: usize, c: usize) -> usize {
    r * 15 + c
}

/** 成五窗去掉补点后的 4 子指纹(排序字符串,用于四的去重与活四识别) */
fn win_key(w: &[usize], skip: usize) -> String {
    let mut set: Vec<usize> = w.iter().copied().filter(|&x| x != skip).collect();
    set.sort_unstable();
    set.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

/* ---------- 独立暴力禁手判定器(与引擎实现完全不同路) ---------- */
const BDR: [i32; 4] = [0, 1, 1, 1];
const BDC: [i32; 4] = [1, 0, 1, -1];
fn b_at(bd: &[u8; SIZE], r: i32, c: i32) -> i32 {
    if r >= 0 && r < 15 && c >= 0 && c < 15 {
        bd[(r * 15 + c) as usize] as i32
    } else {
        -1
    }
}
fn b_run_len(bd: &[u8; SIZE], r: i32, c: i32, d: usize) -> i32 {
    let p = b_at(bd, r, c);
    let mut n = 1;
    for s in [-1i32, 1] {
        let mut rr = r + BDR[d] * s;
        let mut cc = c + BDC[d] * s;
        while b_at(bd, rr, cc) == p {
            n += 1;
            rr += BDR[d] * s;
            cc += BDC[d] * s;
        }
    }
    n
}
/** 过 (r0,c0) 的「恰好五」窗(5 连、外沿非黑),返回每窗 5 个 cell */
fn b_five_windows(bd: &mut [u8; SIZE], r0: i32, c0: i32, d: usize) -> Vec<Vec<usize>> {
    let mut out = vec![];
    for s in -4..=0 {
        let mut cells = vec![];
        let mut all = true;
        for i in 0..5 {
            let rr = r0 + (s + i) * BDR[d];
            let cc = c0 + (s + i) * BDC[d];
            if b_at(bd, rr, cc) != 1 {
                all = false;
                break;
            }
            cells.push((rr * 15 + cc) as usize);
        }
        if !all {
            continue;
        }
        let br = r0 + (s - 1) * BDR[d];
        let bc = c0 + (s - 1) * BDC[d];
        let ar = r0 + (s + 5) * BDR[d];
        let ac = c0 + (s + 5) * BDC[d];
        if b_at(bd, br, bc) == 1 || b_at(bd, ar, ac) == 1 {
            continue;
        }
        out.push(cells);
    }
    out
}
/** (r0,c0) 已落黑的前提下,该点是否禁手 */
fn brute_inner(bd: &mut [u8; SIZE], r0: i32, c0: i32, depth: u32) -> bool {
    if depth > 8 {
        return true;
    }
    let mut five = false;
    let mut over = false;
    for d in 0..4 {
        let l = b_run_len(bd, r0, c0, d);
        if l == 5 {
            five = true;
        }
        if l >= 6 {
            over = true;
        }
    }
    if five {
        return false;
    }
    if over {
        return true;
    }
    /* 四:逐方向枚举补五点,按成五窗的 4 子集合去重 */
    let mut fours = 0;
    for d in 0..4 {
        let mut sets: Vec<String> = vec![];
        for off in -4..=4 {
            if off == 0 {
                continue;
            }
            let qr = r0 + off * BDR[d];
            let qc = c0 + off * BDC[d];
            if b_at(bd, qr, qc) != 0 {
                continue;
            }
            bd[(qr * 15 + qc) as usize] = 1;
            for w in b_five_windows(bd, r0, c0, d) {
                let key = win_key(&w, (qr * 15 + qc) as usize);
                if !sets.contains(&key) {
                    sets.push(key);
                }
            }
            bd[(qr * 15 + qc) as usize] = 0;
        }
        fours += sets.len() as i32;
    }
    if fours >= 2 {
        return true;
    }
    /* 三:枚举补活四点 p(p 不成五、p 非禁手),看是否出现含 p 的活四 */
    let mut threes = 0;
    for d in 0..4 {
        if threes >= 2 {
            break;
        }
        let mut found = false;
        for off in -3..=3 {
            if off == 0 || found {
                continue;
            }
            let pr = r0 + off * BDR[d];
            let pc = c0 + off * BDC[d];
            if b_at(bd, pr, pc) != 0 {
                continue;
            }
            bd[(pr * 15 + pc) as usize] = 1;
            let mut bad = false;
            for dd in 0..4 {
                if b_run_len(bd, pr, pc, dd) >= 5 {
                    bad = true;
                    break;
                }
            }
            let mut ok = false;
            if !bad {
                let mut sets: Vec<String> = vec![];
                for off2 in -4..=4 {
                    if off2 == 0 {
                        continue;
                    }
                    let qr = r0 + off2 * BDR[d];
                    let qc = c0 + off2 * BDC[d];
                    if b_at(bd, qr, qc) != 0 {
                        continue;
                    }
                    bd[(qr * 15 + qc) as usize] = 1;
                    for w in b_five_windows(bd, r0, c0, d) {
                        if w.contains(&((pr * 15 + pc) as usize)) {
                            sets.push(win_key(&w, (qr * 15 + qc) as usize));
                        }
                    }
                    bd[(qr * 15 + qc) as usize] = 0;
                }
                ok = sets.iter().any(|k| sets.iter().filter(|k2| *k2 == k).count() >= 2);
            }
            if ok {
                ok = !brute_inner(bd, pr, pc, depth + 1);
            }
            bd[(pr * 15 + pc) as usize] = 0;
            if ok {
                found = true;
            }
        }
        if found {
            threes += 1;
        }
    }
    threes >= 2
}
fn brute_forbidden(bd_in: &[u8; SIZE], c: usize) -> bool {
    let mut bd = *bd_in;
    bd[c] = 1;
    brute_inner(&mut bd, (c / 15) as i32, (c % 15) as i32, 0)
}
fn brute_made_five(bd_in: &[u8; SIZE], c: usize, mode: i32) -> bool {
    let bd = *bd_in;
    let p = bd[c];
    let side = p - 1;
    let exact = mode == RENJU && side == BLACK;
    for d in 0..4 {
        let l = b_run_len(&bd, (c / 15) as i32, (c % 15) as i32, d);
        if if exact { l == 5 } else { l >= 5 } {
            return true;
        }
    }
    false
}

/* ---------- 简单 LCG(固定种子,失败可复现) ---------- */
struct Rnd(u64);
impl Rnd {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / ((1u64 << 31) as f64)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_f64() * n as f64) as usize % n.max(1)
    }
}

fn search_now(mode: i32, depth: i32, nodes: u64) -> (i32, i32, bool) {
    unsafe {
        search_best(mode, depth, nodes, u64::MAX);
        let st = &*std::ptr::addr_of!(aether_renju::search::S);
        (st.best, st.best_score, st.mate)
    }
}

/* ===== 1. 胜负判定 ===== */
#[test]
fn win() {
    let _g = lock();
    unsafe {
        pos(&[(7, ".......xxxxx....")]);
        assert!(made_five(cell(7, 9), FREE));
        assert!(made_five(cell(7, 9), RENJU));
        let mut line = [0usize; 15];
        assert_eq!(check_win(cell(7, 9), FREE, &mut line), 5);
        for &c in &[cell(7, 7), cell(7, 8), cell(7, 10), cell(7, 11)] {
            assert!(made_five(c, RENJU), "five member {c}");
        }
        let (bd, _) = pos(&[(7, "......xxxxxx....")]);
        assert!(made_five(cell(7, 8), FREE));
        assert!(!made_five(cell(7, 8), RENJU));
        assert_eq!(check_win(cell(7, 8), FREE, &mut line), 6);
        pos(&[(7, "......oooooo....")]);
        assert!(made_five(cell(7, 8), FREE));
        assert!(made_five(cell(7, 8), RENJU));
        pos(&[
            (3, "x..............."),
            (4, ".x.............."),
            (5, "..x............."),
            (6, "...x............"),
            (7, "....x..........."),
        ]);
        assert!(made_five(cell(5, 2), RENJU));
        pos(&[
            (3, "............x..."),
            (4, "...........x...."),
            (5, "..........x....."),
            (6, ".........x......"),
            (7, "........x......."),
        ]);
        assert!(made_five(cell(5, 10), RENJU));
        pos(&[(0, "xxxxx...........")]);
        assert!(made_five(cell(0, 2), RENJU));
        let (bd, _) = pos(&[(7, "......xxxxxx...."), (8, "ooooo..........."), (9, "....x...x...o...")]);
        for &c in &[cell(7, 8), cell(8, 2), cell(9, 4)] {
            assert_eq!(made_five(c, FREE), brute_made_five(&bd, c, FREE), "free cross-check {c}");
            assert_eq!(made_five(c, RENJU), brute_made_five(&bd, c, RENJU), "renju cross-check {c}");
        }
    }
}

/* ===== 2. 禁手(手工构造用例) ===== */
#[test]
fn forbidden() {
    let _g = lock();
    unsafe {
        let (_, star) = pos(&[(7, ".......xxxxx*...")]);
        assert!(is_forbidden(star.unwrap(), 0), "overline right");
        let (_, star) = pos(&[(7, "......*xxxxx....")]);
        assert!(is_forbidden(star.unwrap(), 0), "overline left");
        let (_, star) = pos(&[(7, "....xxxx*.......")]);
        assert!(!is_forbidden(star.unwrap(), 0), "five exempts");
        let (_, star) = pos(&[
            (5, ".........x......"),
            (6, ".........x......"),
            (7, ".....xxxx*......"),
            (8, ".........x......"),
            (9, ".........x......"),
            (10, ".........x......"),
            (11, ".........x......"),
        ]);
        assert!(!is_forbidden(star.unwrap(), 0), "five beats overline");
        let (_, star) = pos(&[
            (4, ".......x........"),
            (5, ".......x........"),
            (6, ".......x........"),
            (7, "....xxx*........"),
        ]);
        assert!(is_forbidden(star.unwrap(), 0), "double four");
        let (_, star) = pos(&[(7, "...xxx*xxx......")]);
        assert!(is_forbidden(star.unwrap(), 0), "same-line double four");
        let (_, star) = pos(&[(7, ".....xxx*.......")]);
        assert!(!is_forbidden(star.unwrap(), 0), "open four is one four");
        let (_, star) = pos(&[(6, ".......x........"), (7, "...oxxx*........"), (8, ".......x........")]);
        assert!(!is_forbidden(star.unwrap(), 0), "four-three allowed");
        let (_, star) = pos(&[(6, ".......x........"), (7, ".....xx*........"), (8, ".......x........")]);
        assert!(is_forbidden(star.unwrap(), 0), "double three");
        let (_, star) = pos(&[(6, ".......x........"), (7, ".....xx*o......."), (8, ".......x........")]);
        assert!(!is_forbidden(star.unwrap(), 0), "fake three");
        let (_, star) = pos(&[(5, "......x........."), (6, "......x........."), (7, "....x.*x........")]);
        assert!(is_forbidden(star.unwrap(), 0), "double jump three");
        let (_, star) = pos(&[
            (6, ".....x.x........"),
            (7, "....x.x*........"),
            (8, ".....x.x........"),
            (9, ".....x.........."),
        ]);
        assert!(!is_forbidden(star.unwrap(), 0), "recursive fake three");
        let (_, star) = pos(&[(6, ".....x.x........"), (7, "....x.x*........"), (8, ".....x.x........")]);
        assert!(is_forbidden(star.unwrap(), 0), "control: real double three");
        let (_, star) = pos(&[(0, "*xxxxx..........")]);
        assert!(is_forbidden(star.unwrap(), 0), "edge overline");
        let (_, star) = pos(&[(0, "xxxx*...........")]);
        assert!(!is_forbidden(star.unwrap(), 0), "edge exact five");
        let (_, star) = pos(&[(7, ".........xxxxx*.")]);
        assert!(is_forbidden(star.unwrap(), 0), "right edge overline");
        let (_, star) = pos(&[(7, "..........xxxx*.")]);
        assert!(!is_forbidden(star.unwrap(), 0), "right edge exact five");
        let (bd, star) = pos(&[(6, ".......o........"), (7, ".....oo*........"), (8, ".......o........")]);
        assert!(!is_forbidden(star.unwrap(), 0), "white never forbidden");
        let mut cells = [0i32; SIZE + 1];
        let n = legal_moves(&mut cells, BLACK, FREE);
        assert!((0..n).any(|i| cells[i] as usize == star.unwrap()), "legal in free mode");
    }
}

/* ===== 3. 禁手对拍(引擎 vs 暴力判定器) ===== */
#[test]
fn forbidden_fuzz() {
    let _g = lock();
    unsafe {
        let mut rnd = Rnd(0x2f6e2b1);
        let mut checked = 0usize;
        let mut mismatches = 0usize;
        for _t in 0..60 {
            let mut bd = [0u8; SIZE];
            let n = 3 + rnd.below(12);
            for i in 0..n {
                let c = rnd.below(SIZE);
                bd[c] = if i % 2 == 0 { 1 } else { 2 };
            }
                let mut five = false;
            for c in 0..SIZE {
                if bd[c] != 0 && brute_made_five(&bd, c, FREE) {
                    five = true;
                    break;
                }
            }
            if five {
                continue;
            }
            sync_position(&bd);
            for c in 0..SIZE {
                if bd[c] != 0 {
                    continue;
                }
                let eng = is_forbidden(c, 0);
                let bru = brute_forbidden(&bd, c);
                checked += 1;
                if eng != bru {
                    mismatches += 1;
                    if mismatches <= 3 {
                        eprintln!("forbidden mismatch cell={c} engine={eng} brute={bru}");
                    }
                }
            }
        }
        assert!(checked > 3000, "enough samples ({checked})");
        assert_eq!(mismatches, 0, "engine forbidden matches brute judge");
    }
}

/* ===== 4. 增量状态一致性 ===== */
#[test]
fn state_consistency() {
    let _g = lock();
    unsafe {
        let mut rnd = Rnd(0x51f2a3b);
        for _t in 0..40 {
            new_board();
            let mut mvs = vec![];
            for _ in 0..20 {
                let empties: Vec<usize> = (0..SIZE).filter(|&c| (*p()).bd[c] == 0).collect();
                let c = empties[rnd.below(empties.len())];
                make(c, side_to_move());
                mvs.push(c);
            }
            let before = snapshot();
                let mut probe = rnd.below(SIZE);
            while (*p()).bd[probe] != 0 {
                probe = (probe + 1) % SIZE;
            }
            make(probe, side_to_move());
            unmake(probe, (*p()).stones as u8 & 1 ^ 1);
            let after = snapshot();
            assert_eq!(before, after, "make/unmake restores state");
                new_board();
            for &c in &mvs {
                make(c, side_to_move());
            }
            assert_eq!(after, snapshot(), "replay consistent");
                for &c in mvs.iter().rev() {
                unmake(c, (*p()).stones as u8 & 1 ^ 1);
            }
            let empty = snapshot();
            assert!(empty.0 == 0 && empty.2 == 0 && empty.1 == [0; SIZE], "back to empty");
        }
    }
}
unsafe fn snapshot() -> (u64, [u16; SIZE], u16, [u8; NW], [u8; NW], [i32; 49], i32, [[i32; 14]; 2]) {
    let q = p();
    let mut vb = 0i32;
    let mut p4 = [[0i32; 14]; 2];
    eval::export_state(&mut vb, &mut p4);
    (q.hk, q.nei, q.stones, q.wb, q.ww, q.cnt, vb, p4)
}

/* ===== 5. 合法着法与重演 ===== */
#[test]
fn moves_and_replay() {
    let _g = lock();
    unsafe {
        new_board();
        let mut cells = [0i32; SIZE + 1];
        assert_eq!(legal_moves(&mut cells, BLACK, RENJU), 9, "empty board = 9");
        assert_eq!(side_to_move(), BLACK);
        let (_, star) = pos(&[(6, ".....x.x........"), (7, "....x.x*........"), (8, ".....x.x........")]);
        let star = star.unwrap();
        let n = legal_moves(&mut cells, BLACK, RENJU);
        assert!(!(0..n).any(|i| cells[i] as usize == star), "double-three filtered");
        let n = legal_moves(&mut cells, BLACK, FREE);
        assert!((0..n).any(|i| cells[i] as usize == star), "free mode keeps it");
        let n = legal_moves(&mut cells, WHITE, RENJU);
        assert!((0..n).any(|i| cells[i] as usize == star), "white unrestricted");
        let mut fp = [0usize; SIZE];
        let nf = forbidden_points(&mut fp);
        assert_eq!(nf, 2, "two forbidden points");
        assert!(fp[..nf].contains(&star) && fp[..nf].contains(&cell(7, 5)), "forbidden set");
        let seq: [u8; 9] = [112, 97, 113, 98, 114, 99, 115, 100, 116]; // 末手黑五连
        new_board();
        assert_eq!(replay_moves(&seq, FREE), -2, "last move five = -2");
        new_board();
        assert_eq!(replay_moves(&[112, 112], FREE), -1, "duplicate rejected");
        new_board();
        assert_eq!(replay_moves(&seq[..4], FREE), BLACK as i32);
        new_board();
        assert_eq!(replay_moves(&seq[..7], FREE), WHITE as i32);
        new_board();
        let mut with_extra = seq.to_vec();
        with_extra.push(60);
        assert_eq!(replay_moves(&with_extra, FREE), -1, "no moves after five");
    }
}

/* ===== 6. 搜索行为 ===== */
#[test]
fn search_behavior() {
    let _g = lock();
    unsafe {
        /* 一手成五:直接找到杀(黑活四已成形,轮黑补一子成五;
         * 旧 JS 版可显式传 side,这里按轮次补一颗白子保持奇偶) */
        new_board();
        for &c in &[112, 97, 113, 98, 114, 99, 115, 100] {
            make(c, side_to_move());
        }
        let (mv, sc, mate) = search_now(FREE, 4, 100_000);
        assert!(mv == 111 || mv == 116, "open four completes (got {mv})");
        assert!(mate && sc > MATE - 200, "mate reported");

        /* 挡对方的冲四:白只有一个成五点,黑必须堵 */
        pos(&[(6, ".oooox.........."), (8, "......xxx.......")]);
        let (mv, _, _) = search_now(FREE, 4, 100_000);
        assert_eq!(mv as usize, cell(6, 0), "must block the five point");

        let (_, star) = pos(&[(6, ".....x.x........"), (7, "....x.x*........"), (8, ".....x.x........")]);
        let star = star.unwrap();
        let (mv, _, _) = search_now(RENJU, 2, 20_000);
        assert_ne!(mv as usize, star, "no forbidden root move");
        let (mv, _, _) = search_now(RENJU, 4, 50_000);
        let mut cells = [0i32; SIZE + 1];
        let n = legal_moves(&mut cells, BLACK, RENJU);
        assert!((0..n).any(|i| cells[i] == mv), "result is legal");

        new_board();
        let (mv, _, _) = search_now(FREE, 2, 20_000);
        let (mr, mc) = ((mv as usize) / N, (mv as usize) % N);
        assert!(
            (5..=9).contains(&mr) && (5..=9).contains(&mc),
            "empty board near center (got {mv})"
        );

        let mut bd3 = [0u8; SIZE];
        let mut i = 0;
        while i < SIZE {
            if i != 112 {
                bd3[i] = if i % 2 == 0 { 2 } else { 1 };
            }
            i += 1;
        }
        sync_position(&bd3);
        let (mv, _, _) = search_now(FREE, 2, 20_000);
        assert_eq!(mv, 112, "only one cell left");
    }
}

/* ===== 6b. VCT 强制胜 ===== */
#[test]
fn vct_finds_forced_win() {
    let _g = lock();
    unsafe {
        /* 黑活三 + 白散子:强制胜(三 → 活四 → 五),
         * VCT 证明搜索应在浅层直接给出杀棋结论 */
        pos(&[
            (7, "......xxx......."), // 黑活三(7,6)(7,7)(7,8),两端(7,5)(7,9)空
            (9, ".........o......"),
            (10, "..........o....."),
            (11, "...........o...."),
        ]);
        /* 7 颗子 = 轮黑 */
        let (mv, sc, mate) = search_now(FREE, 4, 60_000);
        assert!(mate && sc > MATE - 200, "open three converts to forced win (mv={mv} sc={sc})");
        assert!(mv as usize == cell(7, 5) || mv as usize == cell(7, 9), "extends the three");

        /* 黑双四起点:横 (7,3)(7,4)(7,5) + 纵 (4,6)(5,6)(6,6),
         * (7,6) 一步双四,白挡不住双成五点 */
        pos(&[
            (7, "...xxx.........."),
            (4, "......x........."),
            (5, "......x........."),
            (6, "......x........."),
            (0, "o..............."),
            (2, "..o............."),
            (10, "..........o....."),
            (12, "............o..."),
            (14, "..............o."),
            (1, "........o......."),
        ]);
        /* 12 颗子 = 轮黑 */
        let (mv, sc, mate) = search_now(FREE, 4, 60_000);
        assert!(mate && sc > MATE - 200, "double four is a forced win (mv={mv} sc={sc})");
        /* 直接双四点 (7,6) 或先冲四再双四(如 (2,6))都是合法证明线,
         * 断言首着必须是强制着(tier ≥ 3) */
        assert!(crate::search::tier_of(mv as usize, BLACK) >= 3, "starts a forcing attack (mv={mv})");
    }
}

/* ===== 7. 随机对局(双模式) ===== */
#[test]
fn random_games() {
    let _g = lock();
    unsafe {
        let mut rnd = Rnd(0x19760918);
        for t in 0..30 {
            let mode = if t % 2 == 0 { FREE } else { RENJU };
            new_board();
            let mut seq: Vec<u8> = vec![];
            let mut winner: i32 = -1;
            let mut win_cell = 0usize;
            loop {
                let side = side_to_move();
                let mut cells = [0i32; SIZE + 1];
                let n = legal_moves(&mut cells, side, mode);
                if n == 0 {
                    break;
                }
                let c = cells[rnd.below(n)] as usize;
                make(c, side);
                seq.push(c as u8);
                if made_five(c, mode) {
                    winner = side as i32;
                    win_cell = c;
                    break;
                }
            }
            if winner >= 0 {
                let bd: [u8; SIZE] = (*p()).bd;
                assert!(brute_made_five(&bd, win_cell, mode), "game {t}: brute confirms win");
                let mut line = [0usize; 15];
                assert!(check_win(win_cell, mode, &mut line) >= 5, "game {t}: win line");
            }
            /* 序列可被 Worker 重演,棋盘一致 */
            let finished_bd: [u8; SIZE] = (*p()).bd;
            new_board();
            let rr = replay_moves(&seq, mode);
            assert!(rr == -2 || rr >= 0, "game {t}: replay accepts ({rr})");
            assert_eq!((*p()).bd, finished_bd, "game {t}: replay board identical");
            /* 有禁模式黑棋从没走过禁手点(暴力确认) */
            if mode == RENJU {
                new_board();
                let mut ok_seq = true;
                for &c in &seq {
                    if side_to_move() == BLACK && brute_forbidden(&(*p()).bd, c as usize) {
                        ok_seq = false;
                        break;
                    }
                    make(c as usize, side_to_move());
                    if made_five(c as usize, mode) {
                        break;
                    }
                }
                assert!(ok_seq, "game {t}: black sequence has no forbidden move");
            }
        }
    }
}

/* ===== 8. perft ===== */
unsafe fn perft(side: u8, mode: i32, depth: i32) -> u64 {
    if depth <= 0 {
        return 1;
    }
    let mut cells = [0i32; SIZE + 1];
    let n = legal_moves(&mut cells, side, mode);
    if depth == 1 {
        return n as u64;
    }
    let mut total = 0u64;
    for i in 0..n {
        let c = cells[i] as usize;
        make(c, side);
        if !made_five(c, mode) {
            total += perft(side ^ 1, mode, depth - 1);
        }
        unmake(c, side);
    }
    total
}
#[test]
fn perft_counts() {
    let _g = lock();
    unsafe {
        new_board();
        assert_eq!(perft(BLACK, FREE, 1), 9, "empty depth 1 = 9");
        assert_eq!(perft(BLACK, FREE, 2), 9 * 24, "empty depth 2 = 9x24");
        pos(&[(6, ".....x.x........"), (7, "....x.x........."), (8, ".....x.x........")]);
        let mut cells = [0i32; SIZE + 1];
        let free_n = legal_moves(&mut cells, BLACK, FREE);
        let renju_n = legal_moves(&mut cells, BLACK, RENJU);
        assert!(renju_n < free_n, "forbidden filtering ({renju_n} < {free_n})");
    }
}

/* ===== 9. 难度档与评估 ===== */
#[test]
fn levels_and_eval() {
    let _g = lock();
    unsafe {
        new_board();
        /* NNUE 评估:近似反对称(stm 是头输入,允许小残差不对称) */
        let e0 = evaluate(BLACK, FREE);
        assert!((evaluate(WHITE, FREE) + e0).abs() <= 64, "near-antisymmetric on empty board");
        make(112, BLACK);
        make(97, WHITE);
        let eb = evaluate(BLACK, RENJU);
        assert!((eb + evaluate(WHITE, RENJU)).abs() <= 128, "near-antisymmetric after stones");
        assert!(eb.abs() <= 10000, "within cp clamp");
        /* 引擎自报难度参数(api 静态区) */
        let lv = &*std::ptr::addr_of!(api::LEVELS_OUT);
        assert_eq!(api::LEVELS_LEN, 16);
        assert!(lv.iter().step_by(4).all(|&d| d > 0));
        assert!(lv.iter().skip(1).step_by(4).all(|&n| n > 0));
    }
}

/* ===== 10. api 层输出 ===== */
#[test]
fn api_state_and_search() {
    let _g = lock();
    unsafe {
        /* 空盘 */
        assert_eq!(api::state(&[], RENJU), 0);
        let st = &*std::ptr::addr_of!(api::STATE_OUT);
        assert_eq!(st[0], 0, "black to move");
        assert_eq!(st[1], 0, "not over");
        assert_eq!(st[240], 0, "no forbidden");
        /* 一步后轮白;黑禁手点存在(天元一子周围无禁手 → 0) */
        assert_eq!(api::state(&[112], RENJU), 0);
        let st = &*std::ptr::addr_of!(api::STATE_OUT);
        assert_eq!(st[0], 1);
        assert_eq!(st[15 + 112], 1, "stone placed");
        /* 末手成五:over + winner + 连线 */
        let seq: [u8; 9] = [112, 97, 113, 98, 114, 99, 115, 100, 116];
        assert_eq!(api::state(&seq, FREE), 0);
        let st = &*std::ptr::addr_of!(api::STATE_OUT);
        assert_eq!(st[1], 1);
        assert_eq!(st[2], 0, "black won (even index)");
        assert_eq!(st[3], 1, "reason five");
        assert_eq!(st[4], 5, "line length");
        /* 成五后继续走 → 非法 */
        let mut over = seq.to_vec();
        over.push(60);
        assert_eq!(api::state(&over, FREE), -1);
        /* search 走一手并验证输出 */
        assert_eq!(api::search(&seq[..8], FREE, 4, 100_000, u32::MAX as u64), 0);
        let so = &*std::ptr::addr_of!(api::SEARCH_OUT);
        assert!(so[0] >= 0 && so[0] < SIZE as i32, "valid move");
        assert!(so[5] > 0, "root list present");

        /* 可达的双三禁手局面(黑白交替合法序列重演得到):
         * 黑 (6,5)(6,7)(7,4)(7,6)(8,5)(8,7),白散在 0/1 行。
         * 落成后 (7,5) 与 (7,7) 是黑方禁手点(双三)。 */
        let seq2: [u8; 12] = [95, 0, 97, 2, 109, 16, 111, 18, 125, 4, 127, 6];
        assert_eq!(api::state(&seq2, RENJU), 0);
        let st = &*std::ptr::addr_of!(api::STATE_OUT);
        assert_eq!(st[0], 0, "black to move");
        assert_eq!(st[240], 2, "two forbidden points");
        let f0 = st[241] as usize;
        let f1 = st[242] as usize;
        assert!(
            { f0 == cell(7, 5) && f1 == cell(7, 7) } || { f0 == cell(7, 7) && f1 == cell(7, 5) },
            "forbidden set = {{(7,5),(7,7)}}, got {f0},{f1}"
        );
    }
}


/* ===== 15. 评估对拍(外部真值夹具,逐局面一致) ===== */
const HCE_FIXTURES: &str = include_str!("eval_fixtures.txt");

#[test]
fn eval_matches_fixtures() {
    let _g = lock();
    unsafe {
        for line in HCE_FIXTURES.lines() {
            let (head, val) = line.rsplit_once(':').unwrap();
            let want: i32 = val.parse().unwrap();
            let mut it = head.split(' ');
            let rule: i32 = it.next().unwrap().parse().unwrap();
            let mode = if rule == 1 { RENJU } else { FREE };
            eval::set_mode(mode);
            new_board();
            let mut side = BLACK;
            for tok in it {
                let (x, y) = tok.split_once(',').unwrap();
                let cell: usize = y.parse::<usize>().unwrap() * N + x.parse::<usize>().unwrap();
                make(cell, side);
                side ^= 1;
            }
            let stm = side_to_move();
            let got = evaluate(stm, mode);
            assert_eq!(got, want, "line: {line}");
        }
    }
}

/* 增量 VB/P4CNT 与全量重算一致(随机对局,行进中周期对拍 + 撤销) */
#[test]
fn eval_incremental_matches_rebuild() {
    let _g = lock();
    unsafe {
        eval::set_mode(FREE);
        new_board();
        let mut rng: u64 = 0x9E37_79B9;
        let mut history = [0usize; 128];
        let mut hn = 0usize;
        let mut side = BLACK;
        let mut checks = 0;
        let mut undos = 0;
        let mut step = 0;
        while step < 600 {
            step += 1;
            if step % 17 == 0 {
                let (mut vb, mut p4) = (0i32, [[0i32; 14]; 2]);
                eval::export_state(&mut vb, &mut p4);
                eval::rebuild_from_board();
                let (mut vb2, mut p42) = (0i32, [[0i32; 14]; 2]);
                eval::export_state(&mut vb2, &mut p42);
                assert_eq!(vb, vb2, "VB incremental vs refresh at step {step}");
                assert_eq!(p4, p42, "P4CNT incremental vs refresh at step {step}");
                checks += 1;
            }
            if step % 23 == 0 && hn >= 2 {
                for _ in 0..2 {
                    hn -= 1;
                    unmake(history[hn], (*p()).stones as u8 & 1 ^ 1);
                    side ^= 1;
                }
                undos += 1;
            }
            let mut cells = [0i32; SIZE + 1];
            let mut scores = [0i32; SIZE + 1];
            let n = gen_candidates(side, &mut cells, &mut scores);
            if n == 0 || hn >= history.len() {
                break;
            }
            /* 避开成五点,延长对局覆盖更多增量路径 */
            let mut cell = cells[(rng >> 33) as usize % n] as usize;
            let mut tries = 0;
            loop {
                make(cell, side);
                let five = made_five(cell, FREE);
                unmake(cell, side);
                if !five || tries >= n {
                    break;
                }
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                cell = cells[(rng >> 33) as usize % n] as usize;
                tries += 1;
            }
            make(cell, side);
            history[hn] = cell;
            hn += 1;
            if made_five(cell, FREE) {
                break;
            }
            side ^= 1;
        }
        assert!(checks >= 3 && undos >= 3, "paths exercised ({checks}/{undos})");
    }
}
