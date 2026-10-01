/* 原生 NPS 基准:固定中盘局面,按档位节点预算测每秒节点数。 */
use aether_renju::*;
use std::env;
use std::time::Instant;

fn main() {
    let budget: u64 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(600_000);
    let ext: i32 = env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);

    /* 中盘局面(取 bench.mjs 同款:双方向对攻) */
    let mids: [(&str, i32, &[u8]); 2] = [
        ("无禁·中盘", FREE, &[112, 82, 96, 113, 97, 98, 127, 83, 128]),
        ("有禁·中盘", RENJU, &[112, 82, 96, 113, 97, 98, 127, 83, 128]),
    ];
    unsafe {
        api::ar_book_enable(0);
        search::set_ext_budget(ext);
        for (name, mode, moves) in mids {
            let mut total_nodes = 0u64;
            let mut total_ms = 0u128;
            for _rep in 0..3 {
                eval::set_mode(mode);
                new_board();
                for &m in moves {
                    make(m as usize, side_to_move());
                }
                let t0 = Instant::now();
                search::search_best(mode, 10, budget, 60_000);
                total_ms += t0.elapsed().as_millis();
                total_nodes += (*search::s()).nodes;
            }
            println!(
                "{name}: {total_nodes} 节点 / {total_ms}ms = {} NPS(ext={ext},预算 {budget})",
                total_nodes as u128 * 1000 / total_ms.max(1)
            );
        }
    }
}
