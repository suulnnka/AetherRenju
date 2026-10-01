/* 26 开局库测试:表本身的正确性 + 形状匹配的对称完备性。
 *
 * 交叉验证用**独立几何实现**:测试侧用网格坐标 (dc,dr)(右/下为正)
 * 自写 8 对称变换,与引擎的罗盘 (dE,dN) 实现不同路;穷举全部合法
 * (白2,黑3) 偏移对 —— 白2 ∈ 8 邻点、黑3 ∈ 切比雪夫 ≤2 的框内、
 * 两手不重不占锚点,共 8×23 = 184 对 —— 26 开局正是这 184 个位形
 * 在 8 对称下的完全分类(20 个轨道大小 8 + 6 个镜像/转置不动轨道
 * 大小 4):要求 184 对全部命中、同一轨道同名(规范折叠无碰撞)、
 * 26 名全部出现。表的坐标来源与多源对拍记录见 book/README.md。 */
use aether_renju::opening::{name, lookup, policy_move_seeded, CENTER, DIAGONAL, DIRECT};
use aether_renju::N;

const C: i32 = 7; // 天元(h8)的网格行列

fn cell(r: i32, c: i32) -> u8 {
    (r * N as i32 + c) as u8
}

/// 三手序列:锚点 + 两个网格偏移 (dc,dr)
fn seq3(ar: i32, ac: i32, d2: (i32, i32), d3: (i32, i32)) -> [u8; 3] {
    [
        cell(ar, ac),
        cell(ar + d2.1, ac + d2.0),
        cell(ar + d3.1, ac + d3.0),
    ]
}

/// 网格坐标系 (dc,dr) 下的 8 对称(独立实现):4 旋转 + 转置复合
fn sym8(i: usize, dc: i32, dr: i32) -> (i32, i32) {
    let (mut x, mut y) = (dc, dr);
    for _ in 0..(i & 3) {
        let (tx, ty) = (-y, x); // 顺时针 90°
        x = tx;
        y = ty;
    }
    if i >= 4 {
        (x, y) = (y, x); // 转置(沿主对角线翻)
    }
    (x, y)
}

fn lookup3(mv: &[u8; 3]) -> Option<&'static str> {
    lookup(mv).map(|(k, i)| name(k, i))
}

/* ---------- 1. 表的规范自检:折叠侧、无重名、星/月命名规律 ---------- */
#[test]
fn table_shape() {
    assert_eq!(DIRECT.len() + DIAGONAL.len(), 26);
    for (table, diagonal_kind) in [(&DIRECT, false), (&DIAGONAL, true)] {
        let mut names: Vec<_> = table.iter().map(|o| o.name).collect();
        names.sort_unstable();
        let n0 = names.len();
        names.dedup();
        assert_eq!(names.len(), n0, "表内有重名");
        for o in table.iter() {
            let (e, n) = o.d3;
            assert!(e.abs() <= 2 && n.abs() <= 2, "{} 黑3 超 5×5 区", o.name);
            if diagonal_kind {
                assert!(e >= n, "{} 未折叠到上侧(转置)", o.name);
            } else {
                assert!(e >= 0, "{} 未折叠到东半(镜像)", o.name);
            }
            /* 命名规律:黑1、黑3 同线或同对角且中隔一格 → 星 */
            let colinear_gap =
                e.abs().max(n.abs()) == 2 && (e == 0 || n == 0 || e.abs() == n.abs());
            assert_eq!(
                o.name.ends_with('星'),
                colinear_gap,
                "{} 的星/月命名与位置矛盾",
                o.name
            );
        }
        assert_eq!(
            table.iter().filter(|o| o.name.ends_with('星')).count(),
            5,
            "星局数应为 5"
        );
    }
}

/* ---------- 2. 标准坐标逐名对拍(黑1=H8;直指白2=H9、斜指白2=I9) ---------- */
#[test]
fn standard_coordinates() {
    /* 期望的黑3 网格偏移 (dc,dr),按 D1..D13 / I1..I13 标准序 */
    let direct: &[(i32, i32)] = &[
        (0, -2), // D1 寒星 H10
        (1, -2), // D2 溪月 I10
        (2, -2), // D3 疏星 J10
        (1, -1), // D4 花月 I9
        (2, -1), // D5 残月 J9
        (1, 0),  // D6 雨月 I8
        (2, 0),  // D7 金星 J8
        (0, 1),  // D8 松月 H7
        (1, 1),  // D9 丘月 I7
        (2, 1),  // D10 新月 J7
        (0, 2),  // D11 瑞星 H6
        (1, 2),  // D12 山月 I6
        (2, 2),  // D13 游星 J6
    ];
    let diagonal: &[(i32, i32)] = &[
        (2, -2), // I1 长星 J10
        (2, -1), // I2 峡月 J9
        (2, 0),  // I3 恒星 J8
        (2, 1),  // I4 水月 J7
        (2, 2),  // I5 流星 J6
        (1, 0),  // I6 云月 I8
        (1, 1),  // I7 浦月 I7
        (1, 2),  // I8 岚月 I6
        (0, 1),  // I9 银月 H7
        (0, 2),  // I10 明星 H6
        (-1, 1), // I11 斜月 G7
        (-1, 2), // I12 名月 G6
        (-2, 2), // I13 彗星 F6
    ];
    for (table, kind, w2, expect) in [
        (&DIRECT[..], 0usize, (0, -1), direct),
        (&DIAGONAL[..], 1usize, (1, -1), diagonal),
    ] {
        for (i, o) in table.iter().enumerate() {
            let d3 = expect[i];
            let mv = seq3(C, C, w2, d3);
            assert_eq!(
                lookup(&mv),
                Some((kind, i)),
                "{} 应命中第 {} 条(白2 {:?} 黑3 {:?})",
                o.name,
                i,
                w2,
                d3
            );
        }
    }
}

/* ---------- 3. 对称完备性:穷举 (白2,黑3) 合法偏移对 ---------- */
#[test]
fn orbit_consistency_exhaustive() {
    let mut matched = 0;
    let mut names = std::collections::BTreeSet::new();
    let mut pairs = 0;
    for d2e in -1..=1 {
        for d2r in -1..=1 {
            if d2e == 0 && d2r == 0 {
                continue;
            }
            for d3e in -2..=2 {
                for d3r in -2..=2 {
                    if (d3e == 0 && d3r == 0) || (d3e == d2e && d3r == d2r) {
                        continue;
                    }
                    pairs += 1;
                    let base = lookup3(&seq3(C, C, (d2e, d2r), (d3e, d3r)));
                    /* 8 对称位形必须同名(或同不命中) */
                    for s in 0..8 {
                        let (e2, r2) = sym8(s, d2e, d2r);
                        let (e3, r3) = sym8(s, d3e, d3r);
                        let got = lookup3(&seq3(C, C, (e2, r2), (e3, r3)));
                        assert_eq!(
                            got, base,
                            "位形 ({d2e},{d2r})+({d3e},{d3r}) 对称 s={s} 结果漂移"
                        );
                    }
                    if let Some(nm) = base {
                        matched += 1;
                        names.insert(nm);
                    }
                }
            }
        }
    }
    assert_eq!(pairs, 184, "穷举域 = 8 邻点 × 23 框内点");
    /* 26 名全部出现、合法位形无一漏配(完备分类) */
    assert_eq!(names.len(), 26, "命中开局名数量");
    assert_eq!(matched, 184, "合法位形全部命中");
}

/* ---------- 4. 平移不变 + 三手后沿用 ---------- */
#[test]
fn translation_and_persistence() {
    /* 同一形状(花月)任意位置、任意朝向都点名;锚点含贴角 */
    for &(ar, ac) in &[(C, C), (3, 4), (1, 1), (12, 11), (10, 2)] {
        for s in 0..8 {
            let (e2, r2) = sym8(s, 0, -1); // 直指白2 各朝向
            let (e3, r3) = sym8(s, 1, -1); // 花月黑3
            let mv = seq3(ar, ac, (e2, r2), (e3, r3));
            assert_eq!(
                lookup3(&mv),
                Some("花月"),
                "锚点 ({ar},{ac}) 对称 s={s} 应为花月"
            );
        }
    }
    /* 三手之后整局沿用 */
    let base = seq3(C, C, (1, -1), (1, 1)); // 浦月:白2 I9、黑3 I7
    assert_eq!(lookup3(&base), Some("浦月"));
    let mv = [base[0], base[1], base[2], cell(C, C + 3)];
    assert_eq!(lookup(&mv), Some((1, 6)), "第四手不影响开局名");
}

/* ---------- 5. 书外位形(超出 3×3 / 5×5 区、非法序列) ---------- */
#[test]
fn non_matching() {
    assert_eq!(lookup(&[]), None, "空序列");
    assert_eq!(lookup(&[112]), None, "一手");
    assert_eq!(lookup(&[112, 97]), None, "两手");
    assert_eq!(lookup3(&seq3(C, C, (0, -2), (1, -1))), None, "白2 距离 2(超 3×3)");
    assert_eq!(lookup3(&seq3(C, C, (3, 0), (0, -1))), None, "白2 距离 3");
    assert_eq!(lookup3(&seq3(C, C, (0, -1), (3, 0))), None, "黑3 距离 3(超 5×5)");
    assert_eq!(lookup3(&seq3(C, C, (1, -1), (3, 3))), None, "斜指黑3 距离 3");
    assert_eq!(lookup(&[225, 97, 98]), None, "落点越界");
    assert_eq!(lookup(&[112, 112, 98]), None, "重复落点");
}

/* ---------- 6. 开局策略(trivial bestmove) ----------
 * 域内给点、书外 None;给出的黑3 必与前三手构成 26 开局之一。 */
#[test]
fn policy_domain() {
    /* 空盘:天元,与种子无关 */
    for seed in 0..8u64 {
        assert_eq!(policy_move_seeded(&[], seed), Some(CENTER));
    }
    /* 一子:黑1 的盘内 8 邻点(贴角锚点只剩 3 个,也都在邻位) */
    let near = |mv: usize, a: usize| -> bool {
        let (mr, mc) = ((mv / N) as i32, (mv % N) as i32);
        let (ar, ac) = ((a / N) as i32, (a % N) as i32);
        (mr - ar).abs() <= 1 && (mc - ac).abs() <= 1 && mv != a
    };
    for seed in 0..64u64 {
        let mv = policy_move_seeded(&[112], seed).unwrap();
        assert!(near(mv, 112), "天元邻点:{}", mv);
        let mv = policy_move_seeded(&[0], seed).unwrap();
        assert!(near(mv, 0), "角点邻点:{}", mv);
    }
    /* 两子(白紧邻黑):黑3 在黑1 的 5×5 区内、不与前两手重合,
     * 且构成的三手必命中 26 开局 */
    for seed in 0..256u64 {
        let mv = policy_move_seeded(&[112, 97], seed).unwrap();
        let (mr, mc) = ((mv / N) as i32, (mv % N) as i32);
        assert!((mr - C).abs() <= 2 && (mc - C).abs() <= 2, "5×5 区内:{}", mv);
        assert_ne!(mv, 112);
        assert_ne!(mv, 97);
        assert!(lookup(&[112, 97, mv as u8]).is_some(), "必成 26 开局:{}", mv);
        /* 离心锚点同规则 */
        let mv = policy_move_seeded(&[14, 29], seed).unwrap(); /* (0,14)+(1,14) */
        let (mr, mc) = ((mv / N) as i32, (mv % N) as i32);
        assert!((mr - 0).abs() <= 2 && (mc - 14).abs() <= 2, "边缘 5×5:{}", mv);
    }
    /* 书外 / 非法 → None(回退搜索) */
    assert_eq!(policy_move_seeded(&[112, 130], 1), None, "白2 不紧邻");
    assert_eq!(policy_move_seeded(&[112, 97, 98], 1), None, "已满三手");
    assert_eq!(policy_move_seeded(&[225, 97], 1), None, "落点越界");
    assert_eq!(policy_move_seeded(&[112, 112], 1), None, "重复落点");
}
