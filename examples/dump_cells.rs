/* 临时对拍工具:逐格打印 4 向线形(黑|白<<4),与外部真值 diff。 */
use aether_renju::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: dump_cells <rule> <x,y> [x,y ...]");
        std::process::exit(1);
    }
    let mode = if args[1].parse::<i32>().unwrap() == 1 { RENJU } else { FREE };
    unsafe {
        eval::set_mode(mode);
        new_board();
        let mut side = BLACK;
        for tok in &args[2..] {
            let (x, y) = tok.split_once(',').unwrap();
            let cell: usize = y.parse::<usize>().unwrap() * N + x.parse::<usize>().unwrap();
            make(cell, side);
            side ^= 1;
        }
        eval::ensure_init();
        // DIR_KIND 是私有的;借 export 不可行 → 直接重放输出(改用 debug 接口)
        eval::debug_dump_cells();
    }
}
