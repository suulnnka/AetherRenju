/* 原生 piskvork 最小客户端:BOARD 喂局面 → 打印应着。
 * 用法:gomocup <ms/move>;stdin 协议子集:START/INFO/BOARD/DONE/END。
 * 局面经 sync_position 按 bd 重建(与落子顺序无关),行棋方 = 子数奇偶。 */
use aether_renju::*;
use std::env;
use std::io::{BufRead, BufReader, Write};

fn read_line(reader: &mut BufReader<std::io::StdinLock<'static>>) -> String {
    let mut s = String::new();
    reader.read_line(&mut s).unwrap();
    s.trim().to_string()
}

fn main() {
    let ms: u64 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let mut reader = BufReader::new(std::io::stdin().lock());
    let mut out = std::io::stdout();
    let mut mode = FREE;
    unsafe { api::ar_book_enable(0) };

    loop {
        let cmd = read_line(&mut reader);
        if cmd.is_empty() || cmd == "END" {
            break;
        }
        if cmd.starts_with("START") {
            let _ = writeln!(out, "OK");
            let _ = out.flush();
        } else if cmd.starts_with("INFO") {
            if let Some(v) = cmd.strip_prefix("INFO rule ") {
                mode = if v.trim() == "4" { RENJU } else { FREE };
            }
        } else if cmd == "BOARD" {
            let mut bd = [0u8; SIZE];
            loop {
                let row = read_line(&mut reader);
                if row == "DONE" || row.is_empty() {
                    break;
                }
                let mut it = row.split(',');
                let (Some(x), Some(y), Some(c)) = (it.next(), it.next(), it.next()) else {
                    continue;
                };
                let (Ok(x), Ok(y), Ok(c)) = (
                    x.trim().parse::<i32>(),
                    y.trim().parse::<i32>(),
                    c.trim().parse::<u8>(),
                ) else {
                    continue;
                };
                if (0..15).contains(&x) && (0..15).contains(&y) && (1..=2).contains(&c) {
                    bd[(y * 15 + x) as usize] = c;
                }
            }
            unsafe {
                eval::set_mode(mode);
                sync_position(&bd);
                search::search_best(mode, 10, u32::MAX as u64, ms);
                let mv = (*search::s()).best;
                if mv >= 0 {
                    let _ = writeln!(out, "{},{}", mv % 15, mv / 15);
                } else {
                    let _ = writeln!(out, "-1,-1");
                }
            }
            let _ = out.flush();
        }
    }
}
