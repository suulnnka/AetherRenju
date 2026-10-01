/* 原生 A/B 对拍器:同进程内两种延伸预算互弈(搜索态每手清零,公平)。
 * 用法:match_native <budgetA> <budgetB> <games/rule> <nodes/move> <seed>
 * 输出两行:无禁/有禁比分(A 视角)。 */
use aether_renju::*;
use std::env;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn play(rule: i32, budget_a: i32, budget_b: i32, games: usize, nodes: u64, rng: &mut Rng) {
    let mut wins = [0usize; 2];
    let mut draws = 0usize;
    for g in 0..games {
        let a_first = g % 2 == 0;
        let open_n = 2 + rng.below(5) as usize;
        let mut cells: Vec<usize> = Vec::new();
        let mut moves: Vec<usize> = Vec::new();
        for _ in 0..open_n {
            let mut c = usize::MAX;
            for _t in 0..50 {
                let r = 5 + rng.below(5) as usize;
                let cc = 5 + rng.below(5) as usize;
                let cell = r * N + cc;
                if !cells.contains(&cell) {
                    c = cell;
                    break;
                }
            }
            if c == usize::MAX {
                break;
            }
            cells.push(c);
            moves.push(c);
        }
        unsafe {
            eval::set_mode(rule);
            new_board();
            for &m in &moves {
                make(m, side_to_move());
            }
        }
        let mut winner = -1i32;
        while moves.len() < 160 {
            unsafe {
                if (*p()).stones >= SIZE as u16 {
                    break;
                }
                let side = side_to_move();
                let is_a = (moves.len() % 2 == 0) == a_first;
                search::set_ext_budget(if is_a { budget_a } else { budget_b });
                search::search_best(rule, 10, nodes, 60_000);
                let mv = (*search::s()).best;
                if mv < 0 {
                    winner = if is_a { 1 } else { 0 };
                    break;
                }
                make(mv as usize, side);
                moves.push(mv as usize);
                if made_five(mv as usize, rule) {
                    winner = if is_a { 0 } else { 1 };
                    break;
                }
            }
        }
        if winner < 0 {
            draws += 1;
        } else {
            wins[winner as usize] += 1;
        }
    }
    println!(
        "{}: A({}) {} : {} B({})(和 {})",
        if rule == FREE { "无禁" } else { "有禁" },
        budget_a,
        wins[0],
        wins[1],
        budget_b,
        draws
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: match_native <budgetA> <budgetB> <games/rule> <nodes> <seed>");
        std::process::exit(1);
    }
    let budget_a: i32 = args[1].parse().unwrap();
    let budget_b: i32 = args[2].parse().unwrap();
    let games: usize = args[3].parse().unwrap();
    let nodes: u64 = args[4].parse().unwrap();
    let seed: u64 = args[5].parse().unwrap();

    unsafe { api::ar_book_enable(0) };

    let mut rng = Rng(seed);
    play(FREE, budget_a, budget_b, games, nodes, &mut rng);
    play(RENJU, budget_a, budget_b, games, nodes, &mut rng);
}
