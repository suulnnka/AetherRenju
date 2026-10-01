/* 基准:node bench/bench.mjs [nps|moves|forbidden]
 * 口径与旧版一致(节点预算为主,同机可复现),引擎为 wasm。 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { performance } from 'node:perf_hooks';

const here = dirname(fileURLToPath(import.meta.url));
const bytes = readFileSync(join(here, '../wasm/aether_renju.wasm'));
const { instance } = await WebAssembly.instantiate(bytes, { env: { now: () => performance.now() } });
const EX = instance.exports;
EX.ar_book_enable?.(0); /* 基准量搜索:关掉前三手的开局策略路径 */
const mem = EX.memory.buffer;
const IN_ADDR = EX.ar_in_ptr();
const IN = new Uint8Array(mem, IN_ADDR, 257);
const STATE = new Int32Array(mem, EX.ar_state_ptr(), EX.ar_state_len());
const SEARCH = new Int32Array(mem, EX.ar_search_ptr(), EX.ar_search_len());
const LEVELS = new Int32Array(mem, EX.ar_levels_ptr(), EX.ar_levels_len());
const fmt = (n) => n.toLocaleString('en-US');

/* 固定局面(与旧版同源):黑白交替、安静、无现成活三/冲四,
 * 否则搜索瞬间算到杀棋,预算跑不满,测不出真实 NPS。 */
const POSITIONS = {
  空盘: [],
  早期: [112, 97, 113, 81, 128, 99, 82, 67],
  中盘: [112, 97, 113, 81, 96, 83, 82, 99, 66, 67, 143, 88, 58, 172, 30, 100, 152, 158],
};

const mode = process.argv[2] || 'nps';

if (mode === 'nps') {
  console.log('档位    节点预算    实际节点     耗时      NPS       深度  评分');
  for (let i = 0; i < LEVELS.length; i += 4) {
    const [depth, nodes, ms] = [LEVELS[i], LEVELS[i + 1], LEVELS[i + 2]];
    IN.set(POSITIONS.中盘);
    const t = performance.now();
    EX.ar_search(IN_ADDR, POSITIONS.中盘.length, 0, depth, nodes, ms);
    const wall = Math.max(performance.now() - t, 1);
    const n = SEARCH[3];
    console.log(
      `${'初级 中级 高级 大师'.split(' ')[i / 4].padEnd(4)} ${String(fmt(nodes)).padStart(10)} ${String(fmt(n)).padStart(11)} ` +
      `${String(Math.round(wall) + 'ms').padStart(8)} ${String(fmt(Math.round(n / wall * 1000))).padStart(10)} ` +
      `${String(SEARCH[2]).padStart(5)}  ${SEARCH[1]}`);
  }
} else if (mode === 'moves') {
  const depth = Number(process.argv[3] || 6);
  console.log(`固定深度 ${depth} 的最佳着法(改搜索/评估后用来对拍)\n`);
  for (const [name, moves] of Object.entries(POSITIONS)) {
    for (const rule of [0, 1]) {
      IN.set(moves);
      const t = performance.now();
      EX.ar_search(IN_ADDR, moves.length, rule, depth, 2_000_000, 15000);
      const wall = Math.max(performance.now() - t, 1);
      console.log(
        `  ${name}·${rule === 0 ? '无禁' : '有禁'}  点 ${String(SEARCH[0]).padStart(3)} 分 ${String(SEARCH[1]).padStart(7)}` +
        `  深度 ${SEARCH[2]}  ${fmt(SEARCH[3])} 节点 / ${Math.round(wall)}ms`);
    }
  }
} else if (mode === 'forbidden') {
  /* 禁手吞吐:中盘局面下全盘标禁手点,重复多轮(缓存热口径) */
  const ROUNDS = 200;
  IN.set(POSITIONS.中盘);
  const t0 = performance.now();
  for (let i = 0; i < ROUNDS; i++) EX.ar_state(IN_ADDR, POSITIONS.中盘.length, 1);
  const wall = Math.max(performance.now() - t0, 1);
  console.log(`重演 + 全盘禁手标记 × ${ROUNDS} 轮 / ${Math.round(wall)}ms` +
    ` → ${fmt(Math.round(ROUNDS / wall * 1000))} 次/s(禁手点 ${STATE[240]} 个)`);
  console.log('注:同局面重复判定走引擎内缓存,此数字是「全盘标禁手点」场景的吞吐口径。');
} else {
  console.log('用法:node bench/bench.mjs [nps|moves|forbidden]');
}
