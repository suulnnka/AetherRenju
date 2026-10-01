/* DAgger 自弈数据生成:引擎自对弈(固定节点预算 + 随机开局),
 * 输出 DISTILL1 格式局面文件(教师标签全零,交由外部 labeldump
 * 用 mix9svq 网络标注 WDL)。
 * 用法:selfplay <out.bin> <rule 0|1> <games> <nodes/move> <seed> */
use aether_renju::*;
use std::env;
use std::io::Write;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
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

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: selfplay <out.bin> <rule 0|1> <games> <nodes/move> <seed>");
        std::process::exit(1);
    }
    let mode = if args[2].parse::<i32>().unwrap() == 1 { RENJU } else { FREE };
    let games: usize = args[3].parse().unwrap();
    let nodes: u64 = args[4].parse().unwrap();
    let mut rng = Rng(args[5].parse().unwrap());

    let mut out: Vec<u8> = Vec::with_capacity(1 << 24);
    out.extend_from_slice(b"DISTILL1");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(15u8);
    out.push(if mode == RENJU { 2u8 } else { 0u8 });
    out.extend_from_slice(&(games as u32).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // 局面数占位,收尾回填
    let pos_count_off = out.len() - 8;

    let mut total_positions: u64 = 0;
    let mut board = [0u8; SIZE];
    let mut pos_buf: Vec<u8> = Vec::with_capacity(718 * 64);

    for _g in 0..games {
        unsafe {
            eval::set_mode(mode);
            new_board();
        }
        /* 随机开局:2-6 手中心区随机着 */
        let open_n = 2 + (rng.below(5) as usize);
        let mut cells: Vec<usize> = Vec::new();
        let mut history: Vec<usize> = Vec::new();
        for _ in 0..open_n {
            let mut c = usize::MAX;
            for _try in 0..50 {
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
            history.push(c);
            unsafe { make(c, side_to_move()) };
        }

        let mut result: u8 = 0; // 1 黑胜 / 2 白胜 / 0 和
        let mut winner_five = false;
        let ply_cap = history.len() + 140;
        while history.len() < ply_cap {
            unsafe {
                if (*p()).stones >= SIZE as u16 {
                    break;
                }
                let side = side_to_move();
                search::search_best(mode, 10, nodes, 60_000);
                let mv = (*search::s()).best;
                if mv < 0 {
                    result = if side == BLACK { 2 } else { 1 };
                    break;
                }
                make(mv as usize, side);
                history.push(mv as usize);
                if made_five(mv as usize, mode) {
                    winner_five = true;
                    result = if side == BLACK { 1 } else { 2 };
                    break;
                }
            }
        }
        let _ = winner_five;

        /* 重放记录每局面(stm = 该局面行棋方) */
        pos_buf.clear();
        unsafe {
            eval::set_mode(mode);
            new_board();
        }
        let mut npos: u16 = 0;
        let mut snapshots: Vec<u16> = Vec::new();
        for (i, &mv) in history.iter().enumerate() {
            /* 记录落子前的局面(含随机开局位形) */
            unsafe {
                let q = p();
                board.copy_from_slice(&q.bd);
            }
            let stm = (i % 2) as u8; // 黑先
            pos_buf.push(stm);
            pos_buf.push(i as u8);
            pos_buf.extend_from_slice(&board);
            pos_buf.extend_from_slice(&[0u8; 29]); // 禁手位图(下游不用)
            pos_buf.extend_from_slice(&[0u8; 12]); // WDL(labeldump 填)
            pos_buf.extend_from_slice(&[0u8; 450]); // 策略(不用)
            npos += 1;
            let side = if i % 2 == 0 { BLACK } else { WHITE };
            unsafe { make(mv, side) };
        }
        /* 终局局面不记 */
        snapshots.push(npos);
        out.extend_from_slice(&npos.to_le_bytes());
        out.push(result);
        out.extend_from_slice(&pos_buf);
        total_positions += npos as u64;
        let _ = &snapshots;
    }

    out[pos_count_off..pos_count_off + 8].copy_from_slice(&total_positions.to_le_bytes());
    let mut f = std::fs::File::create(&args[1]).unwrap();
    f.write_all(&out).unwrap();
    eprintln!(
        "wrote {} games / {} positions -> {}",
        games, total_positions, args[1]
    );
}
