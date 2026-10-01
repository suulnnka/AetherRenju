/* 调参特征导出:遍历 distillgen 数据,逐局面导出
 * (rule, stm, teacher wlr, mask, 当前/上一拍 pcode 稀疏直方图)。
 * 二进制布局见 tools/distill/tune_hce.py 头注释。 */
use aether_renju::*;
use std::env;
use std::fs::File;
use std::io::{BufReader, Read, Write};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: dump_features <out.bin> <in1.bin> [in2.bin ...]");
        std::process::exit(1);
    }
    let mut out_rec = File::create(format!("{}.rec", args[1])).unwrap();
    let mut out_ent = File::create(format!("{}.ent", args[1])).unwrap();
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 22);
    let mut ebuf: Vec<u8> = Vec::with_capacity(1 << 24);
    let mut total: u32 = 0;

    for path in &args[2..] {
        let mut data = Vec::new();
        BufReader::new(File::open(path).unwrap()).read_to_end(&mut data).unwrap();
        let ngames = u32::from_le_bytes([data[14], data[15], data[16], data[17]]) as usize;
        let rule_code = data[13] as i32;
        let mode = if rule_code == 2 { RENJU } else { FREE };
        let mut off = 26usize;
        let mut hb = [0i16; 3876];
        let mut hw = [0i16; 3876];
        let mut hist_cur: Vec<(u16, i16, i16)> = Vec::new(); // (pcode, 黑计数, 白计数)
        let mut hist_prev: Vec<(u16, i16, i16)> = Vec::new();
        let mut mask_cur = 0usize;

        for _g in 0..ngames {
            let npos = u16::from_le_bytes([data[off], data[off + 1]]) as usize;
            let rec_base = off + 3;
            // 重构着法(相邻局面差 1 子)
            unsafe {
                eval::set_mode(mode);
                new_board();
                let mut prev_board = [0u8; SIZE];
                let mut side = BLACK;
                for i in 0..npos {
                    let o = rec_base + i * 718;
                    let board_off = o + 2;
                    if i > 0 {
                        let mut mv = usize::MAX;
                        for c in 0..SIZE {
                            if data[board_off + c] != prev_board[c] {
                                mv = c;
                                break;
                            }
                        }
                        assert!(mv != usize::MAX);
                        make(mv, side);
                        side ^= 1;
                    }
                    for c in 0..SIZE {
                        prev_board[c] = data[board_off + c];
                    }
                    // 只导出 i>=2 的局面(需要上一拍特征)
                    if i < 2 {
                        if i == 1 {
                            feature_sparse(&mut hist_prev, &mut hb, &mut hw, &mut mask_cur);
                        }
                        continue;
                    }
                    hist_cur.clear();
                    feature_sparse(&mut hist_cur, &mut hb, &mut hw, &mut mask_cur);
                    // label: stm-pov wlr = w − l
                    let val_off = o + 2 + 225 + 29;
                    let w = f32::from_le_bytes([
                        data[val_off],
                        data[val_off + 1],
                        data[val_off + 2],
                        data[val_off + 3],
                    ]);
                    let l = f32::from_le_bytes([
                        data[val_off + 4],
                        data[val_off + 5],
                        data[val_off + 6],
                        data[val_off + 7],
                    ]);
                    let stm = if i % 2 == 0 { 0u8 } else { 1u8 };
                    // 记录头(8B)
                    let r = total;
                    buf.extend_from_slice(&[rule_code as u8, stm, 0, 0]);
                    buf.extend_from_slice(&(w - l).to_le_bytes());
                    buf.extend_from_slice(&(mask_cur as u16).to_le_bytes());
                    // 条目:freestyle 同表异号(fidx = pcode);renju 黑表 +3876 / 白表 +7752
                    write_ents(&mut ebuf, r, rule_code, &hist_cur);
                    write_ents(&mut ebuf, r, rule_code, &hist_prev);
                    total += 1;
                    // prev ← cur(下一拍的上一拍就是本拍)
                    std::mem::swap(&mut hist_prev, &mut hist_cur);
                }
            }
            off = rec_base + npos * 718;
        }
        eprintln!("{}: cumulative {}", path, total);
    }
    out_rec.write_all(&(total as u32).to_le_bytes()).unwrap();
    out_rec.write_all(&buf).unwrap();
    out_ent.write_all(&(ebuf.len() as u32 / 8).to_le_bytes()).unwrap();
    out_ent.write_all(&ebuf).unwrap();
    eprintln!("wrote {} records, {} entries", total, ebuf.len() / 8);
}

/// 条目编码:coef 为 0.5 步进 → i16 ×2;表基址 0/3876/7752。
fn write_ents(buf: &mut Vec<u8>, row: u32, rule: i32, h: &[(u16, i16, i16)]) {
    let base = if rule == 0 { 0u32 } else { 3876 };
    let wbase = if rule == 0 { 0u32 } else { 7752 };
    for (k, cb, cw) in h {
        if rule == 0 {
            let c = cb - cw; // freestyle:同表异号(coef=0.5×差,×2 编码)
            if c != 0 {
                buf.extend_from_slice(&row.to_le_bytes());
                buf.extend_from_slice(&(*k as u16).to_le_bytes());
                buf.extend_from_slice(&c.to_le_bytes());
            }
        } else {
            if *cb != 0 {
                buf.extend_from_slice(&row.to_le_bytes());
                buf.extend_from_slice(&((base + *k as u32) as u16).to_le_bytes());
                buf.extend_from_slice(&(cb).to_le_bytes());
            }
            if *cw != 0 {
                buf.extend_from_slice(&row.to_le_bytes());
                buf.extend_from_slice(&((wbase + *k as u32) as u16).to_le_bytes());
                buf.extend_from_slice(&(-cw).to_le_bytes());
            }
        }
    }
}

unsafe fn feature_sparse(
    out: &mut Vec<(u16, i16, i16)>,
    hb: &mut [i16; 3876],
    hw: &mut [i16; 3876],
    mask: &mut usize,
) {
    eval::feature_dump(hb, hw, mask);
    out.clear();
    for i in 0..3876 {
        if hb[i] != 0 || hw[i] != 0 {
            out.push((i as u16, hb[i], hw[i]));
        }
    }
}

