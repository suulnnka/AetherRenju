/* wasm 冒烟测试(Node):1) 直接驱动 C ABI;2) 经 src/worker.js
 * 走 UI 实际使用的消息契约。完整规则/禁手/搜索测试在 Rust 侧
 * (cargo test --release)。运行:node test/wasm-test.mjs */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { Worker } from 'node:worker_threads';
import { performance } from 'node:perf_hooks';

const here = dirname(fileURLToPath(import.meta.url));
const bytes = readFileSync(join(here, '../wasm/aether_renju.wasm'));

let pass = 0, fail = 0;
const ok = (cond, msg, extra) => {
  if (cond) { pass++; }
  else { fail++; console.log(`  ✗ ${msg}${extra !== undefined ? '  — ' + extra : ''}`); }
};
const eq = (got, want, msg) => ok(got === want, msg, `得到 ${got},期望 ${want}`);
const cell = (r, c) => r * 15 + c;

/* ==================== 1. 直接驱动 wasm(C ABI) ==================== */
const { instance } = await WebAssembly.instantiate(bytes, { env: { now: () => performance.now() } });
const EX = instance.exports;
const mem = EX.memory.buffer;
const IN_ADDR = EX.ar_in_ptr();
const IN = new Uint8Array(mem, IN_ADDR, 257);
const STATE = new Int32Array(mem, EX.ar_state_ptr(), EX.ar_state_len());
const SEARCH = new Int32Array(mem, EX.ar_search_ptr(), EX.ar_search_len());

{
  console.log('\n== wasm ABI ==');
  eq(EX.ar_version(), 2, 'ABI 版本 = 2');

  /* 空盘 state:轮黑、无禁手、未终局 */
  eq(EX.ar_state(IN_ADDR, 0, 1), 0, '空盘 state 正常');
  eq(STATE[0], 0, '空盘轮黑');
  eq(STATE[1], 0, '未终局');
  eq(STATE[240], 0, '无禁手点');

  /* 一步后轮白 */
  IN.set([112]);
  eq(EX.ar_state(IN_ADDR, 1, 1), 0, '一手 state 正常');
  eq(STATE[0], 1, '轮白');
  eq(STATE[15 + 112], 1, '黑子已落');

  /* 末手成五:黑胜,连线 5 */
  const five = [112, 97, 113, 98, 114, 99, 115, 100, 116];
  IN.set(five);
  eq(EX.ar_state(IN_ADDR, five.length, 0), 0, '终局 state 正常');
  eq(STATE[1], 1, '已终局');
  eq(STATE[2], 0, '黑胜');
  eq(STATE[4], 5, '连线长 5');

  /* 成五后继续走 → 非法 */
  IN.set([...five, 60]);
  eq(EX.ar_state(IN_ADDR, five.length + 1, 0), -1, '终局后续走拒绝');

  /* 可达双三局面:轮黑,两个禁手点 */
  const dbl3 = [95, 0, 97, 2, 109, 16, 111, 18, 125, 4, 127, 6];
  IN.set(dbl3);
  eq(EX.ar_state(IN_ADDR, dbl3.length, 1), 0, '双三局面正常');
  eq(STATE[0], 0, '轮黑');
  eq(STATE[240], 2, '两个禁手点');
  ok(
    (STATE[241] === cell(7, 5) && STATE[242] === cell(7, 7))
    || (STATE[241] === cell(7, 7) && STATE[242] === cell(7, 5)),
    '禁手点集合 = (7,5),(7,7)', `${STATE[241]},${STATE[242]}`);

  /* 搜索:黑活三强转(必胜) */
  const open3 = [cell(7, 6), 0, cell(7, 7), 2, cell(7, 8), 4, cell(9, 9), 6];
  IN.set(open3);
  eq(EX.ar_search(IN_ADDR, open3.length, 0, 6, 200000, 2500), 0, '搜索正常返回');
  const mv = SEARCH[0], score = SEARCH[1], flags = SEARCH[4];
  ok(Math.abs(score) > 29800 && (flags & 1), '活三强转报杀', `score=${score}`);
  ok(mv === cell(7, 5) || mv === cell(7, 9), '延伸活三', `mv=${mv}`);
  ok(SEARCH[5] >= 1, '根分数表在场', `rootLen=${SEARCH[5]}`);

  /* 非法序列:重复落点 */
  IN.set([112, 112]);
  eq(EX.ar_search(IN_ADDR, 2, 0, 4, 10000, 1000), -1, '非法序列拒绝');

  /* 开局库(仅显示):花月 = h8,h9,i9;名字经 ptr/len 零拷贝读回 */
  ok(typeof EX.ar_opening === 'function', 'ar_opening 导出在场');
  const flower = [112, cell(6, 7), cell(6, 8)];
  IN.set(flower);
  const oi = EX.ar_opening(IN_ADDR, 3);
  ok(oi >= 1 && oi <= 26, '前三手命中 26 开局', `oi=${oi}`);
  const oname = new TextDecoder().decode(
    new Uint8Array(mem, EX.ar_opening_name_ptr(), EX.ar_opening_name_len()));
  eq(oname, '花月', '开局名 = 花月');
  IN.set([...flower, cell(7, 5)]);
  ok(EX.ar_opening(IN_ADDR, 4) === oi, '三手之后沿用');
  IN.set([112, cell(6, 7)]);
  eq(EX.ar_opening(IN_ADDR, 2), 0, '不足三手不命中');
  eq(EX.ar_opening_name_ptr(), 0, '未命中名字 ptr = 0');
  IN.set([112, cell(4, 7), cell(6, 8)]);       /* 白2 距离 2,书外 */
  eq(EX.ar_opening(IN_ADDR, 3), 0, '书外位形不命中');

  /* 开局策略(trivial bestmove):前三手书内位形直接出着、
   * 不走搜索(flags bit3);书外位形或开关关闭则回退搜索 */
  ok(typeof EX.ar_book_enable === 'function', 'ar_book_enable 导出在场');
  EX.ar_book_enable(1);
  const far = (m) => Math.max(Math.abs((m % 15) - 7), Math.abs(((m / 15) | 0) - 7));
  IN.set([]);
  eq(EX.ar_search(IN_ADDR, 0, 0, 4, 8000, 400), 0, '空盘策略路径正常');
  eq(SEARCH[0], 112, '空盘 → 天元');
  ok((SEARCH[4] & 8) !== 0, '空盘 flags 标记开局策略');
  eq(SEARCH[3], 0, '策略着法无搜索节点');
  IN.set([112]);
  eq(EX.ar_search(IN_ADDR, 1, 0, 4, 8000, 400), 0, '一手策略路径正常');
  ok((SEARCH[4] & 8) !== 0 && SEARCH[3] === 0, '一手也是策略着法');
  const w2 = SEARCH[0];
  ok(w2 !== 112 && far(w2) === 1, '白2 = 黑1 紧邻', `w2=${w2}`);
  IN.set([112, w2]);
  eq(EX.ar_search(IN_ADDR, 2, 0, 4, 8000, 400), 0, '两手策略路径正常');
  ok((SEARCH[4] & 8) !== 0, '紧邻白2 → 策略着法');
  const b3 = SEARCH[0];
  ok(far(b3) <= 2 && b3 !== 112 && b3 !== w2, '黑3 在 5×5 区内且不重复', `b3=${b3}`);
  IN.set([112, w2, b3]);
  ok(EX.ar_opening(IN_ADDR, 3) > 0, '策略黑3 构成 26 开局');
  IN.set([112, 130]);
  eq(EX.ar_search(IN_ADDR, 2, 0, 6, 200000, 2500), 0, '书外两手回退搜索');
  eq(SEARCH[4] & 8, 0, '书外位形无策略标记');
  ok(SEARCH[3] > 0, '回退搜索有节点');
  EX.ar_book_enable(0);
  IN.set([112]);
  eq(EX.ar_search(IN_ADDR, 1, 0, 4, 8000, 400), 0, '关闭策略后正常返回');
  eq(SEARCH[4] & 8, 0, '关闭后无策略标记');
  ok(SEARCH[3] > 0, '关闭后走搜索');
  EX.ar_book_enable(1);
}

/* ==================== 2. Worker 消息契约(src/worker.js 本体) ==================== */
{
  console.log('\n== worker 契约 ==');
  const w = new Worker(new URL('./worker-host.mjs', import.meta.url));
  const ask = (msg) => new Promise((res) => {
    w.once('message', res);
    w.postMessage(msg);
  });

  const pong = await ask({ type: 'ping' });
  ok(pong && pong.type === 'pong' && pong.tag === 'renju-engine-v2', 'ping → pong');

  const lv = await ask({ type: 'levels' });
  ok(lv && lv.type === 'levels' && lv.engine === 'wasm', 'levels 上报');
  eq(lv.levels.length, 4, '四档难度');
  ok(lv.levels.every((x) => x.depth > 0 && x.nodes > 0 && typeof x.name === 'string'), '档位参数齐全');
  eq(lv.default, 2, '默认档 = 高级');

  const st = await ask({ type: 'state', id: 7, moves: [112], mode: 1 });
  ok(st && st.type === 'state' && st.id === 7, 'state 回包形状');
  eq(st.stm, 1, '一手后轮白');
  eq(st.board[112], 1, '棋盘正确');
  eq(st.forbidden.length, 0, '无禁手');

  const bad = await ask({ type: 'state', id: 8, moves: [112, 112], mode: 1 });
  eq(bad.error, 'illegal-sequence', '非法序列 → error');

  /* state 回包的开局名字段(26 开局命中才有;仅显示,不影响 think) */
  const st3 = await ask({ type: 'state', id: 9, moves: [112, cell(6, 7), cell(6, 8)], mode: 1 });
  eq(st3.opening, '花月', 'state 回包带开局名(花月)');
  const st4 = await ask({ type: 'state', id: 10, moves: [112, cell(6, 7), cell(6, 8), 60], mode: 1 });
  eq(st4.opening, '花月', '三手后开局名沿用');
  const st2 = await ask({ type: 'state', id: 11, moves: [112, cell(6, 7)], mode: 1 });
  ok(!('opening' in st2), '未命中不带 opening 字段');
  const stx = await ask({ type: 'state', id: 12, moves: [112, cell(4, 7), cell(6, 8)], mode: 1 });
  ok(!('opening' in stx), '书外位形不带 opening 字段');

  /* 前两手:开局策略着法(trivial bestmove,无搜索统计) */
  const rb = await ask({ id: 3, moves: [112, 97], mode: 0, level: 2 });
  ok(rb && rb.id === 3 && rb.move >= 0 && rb.move < 225, 'think 回包形状', JSON.stringify(rb));
  eq(rb.book, true, '两手局面 = 开局策略着法');
  eq(rb.nodes, 0, '策略着法无节点');
  eq(rb.depth, 0, '策略着法无深度');
  const sb = await ask({ type: 'state', id: 13, moves: [112, 97, rb.move], mode: 1 });
  ok(!!sb.opening, '策略黑3 构成 26 开局', sb.opening);

  /* 第四手起:真搜索,统计齐全 */
  const mid = [112, 97, 113, 81];
  const r = await ask({ id: 4, moves: mid, mode: 0, level: 2 });
  ok(r && r.id === 4 && r.move >= 0 && r.move < 225, '搜索 think 回包形状', JSON.stringify(r));
  eq(r.book, false, '搜索着法不带 book');
  ok(r.nodes > 0 && r.ms >= 0 && typeof r.mate === 'boolean', '统计字段齐全');
  ok(r.depth >= 1 && r.depth <= 6, '深度在档位上限内', `depth=${r.depth}`);

  /* 低难度 jitter:同局面多次思考应出现不止一种选点(弱得可控) */
  const picks = new Set();
  for (let i = 0; i < 12; i++) {
    const rr = await ask({ id: 100 + i, moves: mid, mode: 0, level: 0 });
    picks.add(rr.move);
  }
  ok(picks.size >= 1 && [...picks].every((m) => m >= 0 && m < 225), '低难度选点合法', [...picks].join(','));

  w.terminate();
}

console.log(`\n${fail ? '✗ ' + fail + ' 项失败' : '✓ 全部通过'}(共 ${pass + fail} 项)`);
process.exit(fail ? 1 : 0);
