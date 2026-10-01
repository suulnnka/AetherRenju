/* ============================================================
 * 26 开局库:黑1、白2、黑3 的形状 → 开局名 + 开局策略着法。
 *
 * 连珠标准开局:黑1 天元,白2 落在以天元为中心的 3×3 区(直指 /
 * 斜指各 4 个等价方向),黑3 落在 5×5 区;扣除对称同型后恰 26 种。
 * 两个用途:
 *   ① **命名(显示)**:前三手相对位置(平移 + 8 对称)与 26 表
 *      吻合即给出开局名 —— 不要求黑1 在天元(离心的同型开局同样
 *      点名)。见 lookup / name。
 *   ② **开局策略(前三手着法)**:trivial bestmove ——
 *      开局域内的位形直接给点、不走搜索,书外位形回退搜索。
 *      搜索阶段(第四手起)完全不读开局库。见 policy_move。
 *
 * 表的来源与推导(2026-09-30):台湾连珠教学网「開局」页的
 * 26開局相對位置图逐格解码,与江湖定式口诀的坐标样本(寒星H10 /
 * 疏星J10 / 花月I9 / 雨月I8 / 丘月I7)全部吻合,并按命名规律自检
 * (黑1、黑3 同线或同对角且中隔一格为「星」,其余为「月」:直/斜
 * 各 5 星 8 月,位置全对)。详见 book/README.md。
 * ============================================================ */

use crate::{N, SIZE};

/// 26 开局条目:名字 + 第三手相对黑1 的偏移 (dE, dN)。
/// 偏移在**规范朝向**下给出:直指白2 在正北 (0,1),斜指白2 在
/// 东北 (1,1);直指按左右镜像折叠(dE ≥ 0),斜指按主对角转置
/// 折叠(dE ≥ dN)。
pub struct Opening {
    pub name: &'static str,
    pub d3: (i8, i8),
}

/// 直指开局 D1..D13(白2 = 正北)
pub const DIRECT: [Opening; 13] = [
    Opening { name: "寒星", d3: (0, 2) },
    Opening { name: "溪月", d3: (1, 2) },
    Opening { name: "疏星", d3: (2, 2) },
    Opening { name: "花月", d3: (1, 1) },
    Opening { name: "残月", d3: (2, 1) },
    Opening { name: "雨月", d3: (1, 0) },
    Opening { name: "金星", d3: (2, 0) },
    Opening { name: "松月", d3: (0, -1) },
    Opening { name: "丘月", d3: (1, -1) },
    Opening { name: "新月", d3: (2, -1) },
    Opening { name: "瑞星", d3: (0, -2) },
    Opening { name: "山月", d3: (1, -2) },
    Opening { name: "游星", d3: (2, -2) },
];

/// 斜指开局 I1..I13(白2 = 东北)
pub const DIAGONAL: [Opening; 13] = [
    Opening { name: "长星", d3: (2, 2) },
    Opening { name: "峡月", d3: (2, 1) },
    Opening { name: "恒星", d3: (2, 0) },
    Opening { name: "水月", d3: (2, -1) },
    Opening { name: "流星", d3: (2, -2) },
    Opening { name: "云月", d3: (1, 0) },
    Opening { name: "浦月", d3: (1, -1) },
    Opening { name: "岚月", d3: (1, -2) },
    Opening { name: "银月", d3: (0, -1) },
    Opening { name: "明星", d3: (0, -2) },
    Opening { name: "斜月", d3: (-1, -1) },
    Opening { name: "名月", d3: (-1, -2) },
    Opening { name: "彗星", d3: (-2, -2) },
];

/// 全表(下标即查找结果;直指 0..13,斜指 13..26)
pub const OPENINGS: [&[Opening]; 2] = [&DIRECT, &DIAGONAL];

/// 平面 8 对称(旋转群 + 转置):i<4 = 顺时针 90°×i,
/// i≥4 = 再叠加转置 (e,n)→(n,e)。作用于相对偏移。
#[inline]
fn sym(i: usize, e: i8, n: i8) -> (i8, i8) {
    let (mut e, mut n) = (e, n);
    let mut k = i & 3;
    while k > 0 {
        let (te, tn) = (n, -e); // 90° 顺时针
        e = te;
        n = tn;
        k -= 1;
    }
    if i >= 4 {
        (e, n) = (n, e); // 转置
    }
    (e, n)
}

/// 前三手(落点序列的开头)匹配 26 开局。命中返回 (0=直指/1=斜指,
/// 表内下标);不足三手、落点越界 / 重复、或形状不在 26 表内 → None。
/// 只看形状:与绝对位置、规则模式无关,三手之后也不失效(取序列
/// 前三手,整局沿用)。
pub fn lookup(moves: &[u8]) -> Option<(usize, usize)> {
    if moves.len() < 3 {
        return None;
    }
    let cells = [moves[0] as usize, moves[1] as usize, moves[2] as usize];
    for &c in cells.iter() {
        if c >= SIZE {
            return None;
        }
    }
    if cells[0] == cells[1] || cells[0] == cells[2] || cells[1] == cells[2] {
        return None;
    }
    /* 相对偏移 (dE, dN):东 = 列增,北 = 行减 */
    let off = |a: usize, b: usize| -> (i8, i8) {
        (
            (cells[b] % N) as i8 - (cells[a] % N) as i8,
            (cells[a] / N) as i8 - (cells[b] / N) as i8,
        )
    };
    let o2 = off(0, 1);
    let o3 = off(0, 2);
    /* 把前三手整体转到规范朝向(白2 = 正北/东北)再查表 */
    let mut s = 0;
    while s < 8 {
        let p2 = sym(s, o2.0, o2.1);
        let kind = if p2 == (0, 1) {
            0
        } else if p2 == (1, 1) {
            1
        } else {
            s += 1;
            continue;
        };
        let p3 = sym(s, o3.0, o3.1);
        if let Some(i) = OPENINGS[kind].iter().position(|o| o.d3 == p3) {
            return Some((kind, i));
        }
        s += 1;
    }
    None
}

/// 开局名(UTF-8 静态串,零拷贝;下标越界返回空串)
pub fn name(kind: usize, idx: usize) -> &'static str {
    match OPENINGS.get(kind) {
        Some(&t) => t.get(idx).map_or("", |o| o.name),
        None => "",
    }
}

/* ==================== 开局策略(trivial bestmove)====================
 * 开局位形直接给点、不走搜索(空盘给天元):
 * 前三手落在 26 开局域内的位形由策略直接给点,不进搜索——
 *   空盘      → 天元;
 *   一子      → 其 8 邻点(白2 直指 / 斜指);
 *   两子      → 黑1 的 5×5 区内空点(黑3;须白2 与黑1 紧邻)。
 * 选点在合法形状集合内均匀随机 —— 26 开局都可能开出;书外位形
 * (白2 不紧邻、已满三手)返回 None,调用方回退搜索。
 * 随机源:wasm 用 env.now(worker / 测试 / 基准实例化时都提供),
 * 原生用固定种子(可复现);带种子的变体供测试。 */

/// 天元(h8)
pub const CENTER: usize = 7 * N + 7;

/// splitmix64 输出(种子混合器)
fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 策略随机源:wasm 取 env.now(performance.now),原生固定(可复现)
#[cfg(target_arch = "wasm32")]
fn entropy() -> u64 {
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn now() -> f64;
    }
    let b = unsafe { now() }.to_bits();
    b ^ (b << 21) ^ 0x2026_0930_A5A5_0000
}
#[cfg(not(target_arch = "wasm32"))]
fn entropy() -> u64 {
    0x2026_0930_F00D_0000
}

/// 同上,种子指定(测试 / 复现用)
pub fn policy_move_seeded(moves: &[u8], seed: u64) -> Option<usize> {
    if moves.is_empty() {
        return Some(CENTER); // 空盘:黑1 天元,与种子无关
    }
    if moves.len() > 2 {
        return None;
    }
    for &m in moves {
        if m as usize >= SIZE {
            return None;
        }
    }
    if moves.len() == 2 && moves[0] == moves[1] {
        return None;
    }
    let (r0, c0) = ((moves[0] as usize / N) as i32, (moves[0] as usize % N) as i32);
    let mut s = seed;
    match moves.len() {
        1 => {
            /* 白2:黑1 的盘内 8 邻点 */
            let mut cand = [0usize; 8];
            let mut n = 0usize;
            let mut dr = -1;
            while dr <= 1 {
                let mut dc = -1;
                while dc <= 1 {
                    let (rr, cc) = (r0 + dr, c0 + dc);
                    if (dr != 0 || dc != 0) && rr >= 0 && rr < N as i32 && cc >= 0 && cc < N as i32
                    {
                        cand[n] = (rr * N as i32 + cc) as usize;
                        n += 1;
                    }
                    dc += 1;
                }
                dr += 1;
            }
            Some(cand[(splitmix(&mut s) % n as u64) as usize])
        }
        _ => {
            /* 黑3:黑1 的 5×5 区内空点(白2 须紧邻黑1,否则不在开局域) */
            let b = moves[1] as usize;
            let (r1, c1) = ((b / N) as i32, (b % N) as i32);
            if (r1 - r0).abs() > 1 || (c1 - c0).abs() > 1 {
                return None;
            }
            let mut cand = [0usize; 23]; // 5×5 = 25 − 黑1 − 白2
            let mut n = 0usize;
            let mut dr = -2;
            while dr <= 2 {
                let mut dc = -2;
                while dc <= 2 {
                    let (rr, cc) = (r0 + dr, c0 + dc);
                    if rr >= 0 && rr < N as i32 && cc >= 0 && cc < N as i32
                        && (rr != r0 || cc != c0) && (rr != r1 || cc != c1)
                    {
                        cand[n] = (rr * N as i32 + cc) as usize;
                        n += 1;
                    }
                    dc += 1;
                }
                dr += 1;
            }
            if n == 0 {
                return None;
            }
            Some(cand[(splitmix(&mut s) % n as u64) as usize])
        }
    }
}

/// 开局策略着法(自带随机源);moves 为从空盘起的落点序列。
/// Some = 书内位形的策略着法(调用方不再搜索),None = 回退搜索。
pub fn policy_move(moves: &[u8]) -> Option<usize> {
    policy_move_seeded(moves, entropy())
}
