/* 基准:节点速度 / 固定深度最佳着法回归 / 禁手判定耗时。
 *
 *   node bench/bench.mjs nps        各档位跑满预算,报节点数与 NPS
 *   node bench/bench.mjs moves      固定深度最佳着法(改搜索/改评估后对拍)
 *   node bench/bench.mjs forbidden  禁手判定吞吐(UI 全盘标 × 的成本口径)
 *
 * 口径与 webos 一致:节点预算为主、墙上时间为兜底,同机可复现。
 */
import {
  newBoard, replayMoves, searchBest, isForbidden, legalMoves,
  LEVELS, BLACK, FREE, RENJU,
} from '../src/engine.js';

const mode = process.argv[2] || 'nps';
const fmt = (n) => n.toLocaleString('en-US');

/* 一组固定局面(黑白交替摆出,坐标随意但有攻防含义),把引擎拉离空盘对称区。
 * 刻意只摆到「 pair 为主、无现成活三/冲四」的安静局面,否则搜索瞬间算到杀棋,
 * 节点预算跑不满,测不出真实 NPS。 */
const POSITIONS = {
  空盘: [],
  早期: [112, 97, 113, 81, 128, 99, 82, 67],
  中盘: [112, 97, 113, 81, 96, 83, 82, 99, 66, 67, 143, 88, 58, 172, 30, 100, 152, 158],
};

function boardAfter(moves) {
  const bd = newBoard();
  replayMoves(bd, moves, FREE);
  return bd;
}

if (mode === 'nps') {
  console.log('档位    节点预算    实际节点     耗时      NPS       深度  评分');
  for (const lv of LEVELS) {
    const bd = boardAfter(POSITIONS.中盘);
    const t = Date.now();
    const r = searchBest(bd, BLACK, { mode: FREE, depth: lv.depth, nodes: lv.nodes, ms: lv.ms });
    const ms = Math.max(Date.now() - t, 1);
    console.log(
      `${lv.name.padEnd(5)} ${String(fmt(lv.nodes)).padStart(10)} ${String(fmt(r.nodes)).padStart(11)} ` +
      `${String(ms + 'ms').padStart(8)} ${String(fmt(Math.round(r.nodes / ms * 1000))).padStart(10)} ` +
      `${String(r.depth).padStart(5)}  ${r.score}`);
  }
} else if (mode === 'moves') {
  const depth = Number(process.argv[3] || 6);
  console.log(`固定深度 ${depth} 的最佳着法(改搜索/评估后用来对拍)\n`);
  for (const [name, moves] of Object.entries(POSITIONS)) {
    for (const rule of [FREE, RENJU]) {
      const bd = boardAfter(moves);
      const t = Date.now();
      const r = searchBest(bd, BLACK, { mode: rule, depth, nodes: 2_000_000, ms: 15000 });
      console.log(
        `  ${name}·${rule === FREE ? '无禁' : '有禁'}  点 ${String(r.move).padStart(3)} 分 ${String(r.score).padStart(7)}` +
        `  深度 ${r.depth}  ${fmt(r.nodes)} 节点 / ${Math.max(Date.now() - t, 1)}ms`);
    }
  }
} else if (mode === 'forbidden') {
  /* 禁手判定吞吐:中盘局面下对全部候选点各判一次(缓存冷热各一遍) */
  const bd = boardAfter(POSITIONS.中盘);
  const cells = legalMoves(bd, BLACK, FREE);       // 候选点(未过滤, 无禁口径)
  const t0 = Date.now();
  let hits = 0;
  for (let round = 0; round < 50; round++) {
    for (const c of cells) if (isForbidden(bd, c)) hits++;
  }
  const ms = Math.max(Date.now() - t0, 1);
  console.log(`候选 ${cells.length} 点 × 50 轮 = ${fmt(cells.length * 50)} 次判定 / ${ms}ms` +
    ` → ${fmt(Math.round(cells.length * 50 / ms * 1000))} 次/s(禁手命中 ${hits})`);
  console.log('注:同局面重复判定走缓存,此数字是「全盘标禁手点」场景的吞吐口径。');
}
