/* 五子棋 / 连珠引擎测试:胜负判定 + 禁手用例 + 搜索行为 + 随机对局模糊测试。
 *
 * 运行:node test/engine-test.mjs            全部
 *      node test/engine-test.mjs <小节名>    只跑指定节(--list 看全部)
 *
 * 禁手判定的金标准是第 2 节的**独立暴力判定器**:按 RIF 定义逐点枚举,
 * 与引擎的增量实现完全不同路,两者在随机局面上必须逐点一致。
 */
import {
  BLACK, WHITE, FREE, RENJU, SIZE, MATE,
  newBoard, syncPosition, make, unmake, sideToMove, stoneCount,
  madeFive, checkWin, isForbidden, forbiddenPoints, legalMoves, hasLegalMove,
  replayMoves, searchBest, evaluate, perft, debugState, LEVELS,
} from '../src/engine.js';

/* ---------- 断言框架 ---------- */
const sections = [];
const section = (name, fn) => sections.push({ name, fn });
let pass = 0, fail = 0;
const ok = (cond, msg, extra) => {
  if (cond) pass++;
  else { fail++; console.log(`  ✗ ${msg}${extra ? '  — ' + extra : ''}`); }
};
const eq = (got, want, msg) => ok(got === want, msg, `得到 ${got},期望 ${want}`);

/* ---------- 摆棋助手 ----------
 * 15×15 字符盘:'.' 空 / 'x' 黑 / 'o' 白 / '*' 待测空点。
 * 传对象 { 行号: '该行字符...' },未给的行全空,短行右侧补 '.'。 */
function pos(spec) {
  const bd = new Int8Array(SIZE);
  let star = -1;
  for (const [r, rowStr] of Object.entries(spec)) {
    const ri = Number(r);
    for (let c = 0; c < 15; c++) {
      const ch = rowStr[c] || '.';
      if (ch === 'x') bd[ri * 15 + c] = 1;
      else if (ch === 'o') bd[ri * 15 + c] = 2;
      else if (ch === '*') star = ri * 15 + c;
    }
  }
  syncPosition(bd);
  return { bd, star };
}
const cell = (r, c) => r * 15 + c;

/* ---------- 独立暴力禁手判定器(与引擎实现完全不同路) ----------
 * 按 RIF 定义逐点枚举,零增量、零缓冲复用,慢但直白:
 *   落 x → 五连豁免 / 长连禁手;
 *   四 = 补一子成「恰好五」的形状,同一方向按成五窗的 4 子集合去重;
 *   三 = 存在空点 p,补 p 后出现「过 x 且过 p 的活四」(两个补五点凑同一组 4 子),
 *        且补 p 不成五、补 p 不是禁手(递归)。 */
const BDR = [0, 1, 1, 1], BDC = [1, 0, 1, -1];
const bAt = (bd, r, c) => (r >= 0 && r < 15 && c >= 0 && c < 15 ? bd[r * 15 + c] : -1);

function bRunLen(bd, r, c, d) {
  const p = bAt(bd, r, c);
  let n = 1;
  for (const s of [-1, 1]) {
    let rr = r + BDR[d] * s, cc = c + BDC[d] * s;
    while (bAt(bd, rr, cc) === p) { n++; rr += BDR[d] * s; cc += BDC[d] * s; }
  }
  return n;
}

/** 过 (r0,c0) 的「恰好五」窗(5 连、外沿非黑),返回窗的 5 个 cell 数组 */
function bFiveWindows(bd, r0, c0, d) {
  const out = [];
  for (let s = -4; s <= 0; s++) {
    const cells = [];
    let all = true;
    for (let i = 0; i < 5; i++) {
      const rr = r0 + (s + i) * BDR[d], cc = c0 + (s + i) * BDC[d];
      if (bAt(bd, rr, cc) !== 1) { all = false; break; }
      cells.push(rr * 15 + cc);
    }
    if (!all) continue;
    const br = r0 + (s - 1) * BDR[d], bc = c0 + (s - 1) * BDC[d];
    const ar = r0 + (s + 5) * BDR[d], ac = c0 + (s + 5) * BDC[d];
    if (bAt(bd, br, bc) === 1 || bAt(bd, ar, ac) === 1) continue;
    out.push(cells);
  }
  return out;
}

/** (r0,c0) 已是黑子的前提下,该点是否禁手 */
function bruteInner(bd, r0, c0, depth) {
  if (depth > 8) return true;
  let five = false, over = false;
  for (let d = 0; d < 4; d++) {
    const L = bRunLen(bd, r0, c0, d);
    if (L === 5) five = true;
    if (L >= 6) over = true;
  }
  if (five) return false;
  if (over) return true;

  /* 四:逐方向枚举补五点,按成五窗的 4 子集合去重 */
  let fours = 0;
  for (let d = 0; d < 4; d++) {
    const sets = [];
    for (let off = -4; off <= 4; off++) {
      if (!off) continue;
      const qr = r0 + off * BDR[d], qc = c0 + off * BDC[d];
      if (bAt(bd, qr, qc) !== 0) continue;
      bd[qr * 15 + qc] = 1;
      for (const w of bFiveWindows(bd, r0, c0, d)) {
        const set = w.filter((x) => x !== qr * 15 + qc).sort((a, b) => a - b).join(',');
        if (!sets.includes(set)) sets.push(set);
      }
      bd[qr * 15 + qc] = 0;
    }
    fours += sets.length;
  }
  if (fours >= 2) return true;

  /* 三:枚举补活四点 p(p 不成五、p 非禁手),看是否出现含 p 的活四 */
  let threes = 0;
  for (let d = 0; d < 4 && threes < 2; d++) {
    let found = false;
    for (let off = -3; off <= 3 && !found; off++) {
      if (!off) continue;
      const pr = r0 + off * BDR[d], pc = c0 + off * BDC[d];
      if (bAt(bd, pr, pc) !== 0) continue;
      bd[pr * 15 + pc] = 1;
      let bad = false;
      for (let dd = 0; dd < 4; dd++) {
        if (bRunLen(bd, pr, pc, dd) >= 5) { bad = true; break; }
      }
      /* 先做纯局部的活四成形检查,通过了才递归判 p 的禁手(指数分支只发生在
       * 真的能成活四的点上,稀疏局面几乎不触发,复杂度才压得住) */
      let ok = false;
      if (!bad) {
        const sets = [];
        for (let off2 = -4; off2 <= 4; off2++) {
          if (!off2) continue;
          const qr = r0 + off2 * BDR[d], qc = c0 + off2 * BDC[d];
          if (bAt(bd, qr, qc) !== 0) continue;
          bd[qr * 15 + qc] = 1;
          for (const w of bFiveWindows(bd, r0, c0, d)) {
            if (!w.includes(pr * 15 + pc)) continue;
            sets.push(w.filter((x) => x !== qr * 15 + qc).sort((a, b) => a - b).join(','));
          }
          bd[qr * 15 + qc] = 0;
        }
        const seen = new Map();
        for (const s of sets) seen.set(s, (seen.get(s) || 0) + 1);
        ok = [...seen.values()].some((v) => v >= 2);
      }
      if (ok) ok = !bruteInner(bd, pr, pc, depth + 1);
      bd[pr * 15 + pc] = 0;
      if (ok) { found = true; break; }
    }
    if (found) threes++;
  }
  return threes >= 2;
}

/** bd 必须是可被改写的副本;cell 必须是空点 */
function bruteForbidden(bdIn, cell, depth = 0) {
  const bd = Int8Array.from(bdIn);
  bd[cell] = 1;
  return bruteInner(bd, (cell / 15) | 0, cell % 15, depth);
}

/** 暴力胜负:cell(已落)是否按规则成五 */
function bruteMadeFive(bdIn, cell, mode) {
  const bd = Int8Array.from(bdIn);
  const r0 = (cell / 15) | 0, c0 = cell % 15;
  const p = bd[cell];
  const side = p - 1;
  const exact = mode === RENJU && side === BLACK;
  for (let d = 0; d < 4; d++) {
    const L = bRunLen(bd, r0, c0, d);
    if (exact ? L === 5 : L >= 5) return true;
  }
  return false;
}

/* ==================== 1. 胜负判定 ==================== */
section('win', () => {
  /* 黑恰好五连:两种模式都胜 */
  let { bd, star } = pos({ 7: '.......xxxxx....' });
  eq(madeFive(bd, cell(7, 9), FREE), true, '无禁:黑五连判胜');
  eq(madeFive(bd, cell(7, 9), RENJU), true, '有禁:黑恰好五连判胜');
  eq(checkWin(bd, cell(7, 9)).length, 5, '胜连线长 5');
  for (const c of [cell(7, 7), cell(7, 8), cell(7, 10), cell(7, 11)]) {
    eq(madeFive(bd, c, RENJU), true, `五连中每一点都判胜 (${c})`);
  }

  /* 黑长连:无禁胜,有禁不算胜(禁手) */
  ({ bd, star } = pos({ 7: '......xxxxxx....' }));
  eq(madeFive(bd, cell(7, 8), FREE), true, '无禁:黑六连判胜');
  eq(madeFive(bd, cell(7, 8), RENJU), false, '有禁:黑六连不算五连');
  eq(checkWin(bd, cell(7, 8), FREE).length, 6, '无禁长连的胜连线长 6');

  /* 白长连:两种模式都胜(RIF 9.1:白方长连也算赢) */
  ({ bd, star } = pos({ 7: '......oooooo....' }));
  eq(madeFive(bd, cell(7, 8), FREE), true, '无禁:白六连判胜');
  eq(madeFive(bd, cell(7, 8), RENJU), true, '有禁:白长连也判胜');

  /* 斜向与边界 */
  ({ bd } = pos({ 3: 'x...............', 4: '.x...............'.slice(0, 15), 5: '..x.............', 6: '...x............', 7: '....x...........' }));
  eq(madeFive(bd, cell(5, 2), RENJU), true, '主斜向五连判胜');
  ({ bd } = pos({ 3: '............x...', 4: '...........x....', 5: '..........x.....', 6: '.........x......', 7: '........x.......' }));
  eq(madeFive(bd, cell(5, 10), RENJU), true, '副斜向五连判胜');
  ({ bd } = pos({ 0: 'xxxxx...........' }));
  eq(madeFive(bd, cell(0, 2), RENJU), true, '贴边五连判胜');

  /* 暴力判定器与引擎一致(胜负部分) */
  ({ bd } = pos({ 7: '......xxxxxx....', 8: 'ooooo...........', 9: '....x...x...o...' }));
  for (const c of [cell(7, 8), cell(8, 2), cell(9, 4)]) {
    eq(madeFive(bd, c, FREE), bruteMadeFive(bd, c, FREE), `无禁胜负对拍 (${c})`);
    eq(madeFive(bd, c, RENJU), bruteMadeFive(bd, c, RENJU), `有禁胜负对拍 (${c})`);
  }
});

/* ==================== 2. 禁手(手工构造用例) ==================== */
section('forbidden', () => {
  /* 长连禁手:两端补都成六 */
  let { bd, star } = pos({ 7: '.......xxxxx*...' });
  eq(isForbidden(bd, star), true, '长连禁手(右端补成六)');
  ({ bd, star } = pos({ 7: '......*xxxxx....' }));
  eq(isForbidden(bd, star), true, '长连禁手(左端补成六)');

  /* 五连豁免:补成恰好五,即使别处构成禁手形状也合法 */
  ({ bd, star } = pos({ 7: '....xxxx*.......' }));
  eq(isForbidden(bd, star), false, '补成五连不是禁手');
  /* 一手同时成五 + 别的方向长连:五连优先 */
  ({ bd, star } = pos({
    5: '.........x......', 6: '.........x......',
    7: '.....xxxx*......',
    8: '.........x......', 9: '.........x......', 10: '.........x......', 11: '.........x......',
  }));
  eq(isForbidden(bd, star), false, '成五的同时在另一方向长连:五连豁免');
  eq(madeFive(bd, star, RENJU), false, '(该点尚未落子,不成五)');

  /* 双四:两个方向各一个四 */
  ({ bd, star } = pos({
    4: '.......x........', 5: '.......x........', 6: '.......x........',
    7: '....xxx*........',
  }));
  eq(isForbidden(bd, star), true, '双四禁手(横竖各一四)');

  /* 单线双四:●●●_x_●●●,两个四共享中心点 */
  ({ bd, star } = pos({ 7: '...xxx*xxx......' }));
  eq(isForbidden(bd, star), true, '单线双四禁手');

  /* 活四只算一个四:不构成双四 */
  ({ bd, star } = pos({ 7: '.....xxx*.......' }));
  eq(isForbidden(bd, star), false, '一手活四(两个成五点同一组子)不是双四');

  /* 四三不算禁手(唯一允许的步型) */
  ({ bd, star } = pos({
    6: '.......x........',
    7: '...oxxx*........',
    8: '.......x........',
  }));
  eq(isForbidden(bd, star), false, '四三禁手豁免');

  /* 双三:横竖各一个活三 */
  ({ bd, star } = pos({
    6: '.......x........',
    7: '.....xx*........',
    8: '.......x........',
  }));
  eq(isForbidden(bd, star), true, '双三禁手(两个活三)');

  /* 假活三:一头被白堵死,成不了活四 → 不算三 */
  ({ bd, star } = pos({
    6: '.......x........',
    7: '.....xx*o.......',
    8: '.......x........',
  }));
  eq(isForbidden(bd, star), false, '假活三不算三(一头被堵)');

  /* 跳三也行:两个跳活三同样禁手 */
  ({ bd, star } = pos({
    5: '......x.........',
    6: '......x.........',
    7: '....x.*x........',
  }));
  eq(isForbidden(bd, star), true, '双跳三禁手');

  /* 递归假活三:横向三的唯一补点是禁手(补它成双四),该三为假 → 只剩一个真三,不禁。
   * 去掉让补点变成双四的那颗子(9,5),补点合法,横向三成真 → 双三禁手。 */
  ({ bd, star } = pos({
    6: '.....x.x........',
    7: '....x.x*........',
    8: '.....x.x........',
    9: '.....x..........',
  }));
  eq(isForbidden(bd, star), false, '递归假活三:补点自身禁手 → 三为假');
  ({ bd, star } = pos({
    6: '.....x.x........',
    7: '....x.x*........',
    8: '.....x.x........',
  }));
  eq(isForbidden(bd, star), true, '对照组:补点合法 → 双三成立');

  /* 边角:贴边长连同样禁手;贴边恰好五连合法 */
  ({ bd, star } = pos({ 0: '*xxxxx..........' }));
  eq(isForbidden(bd, star), true, '贴边长连禁手');
  ({ bd, star } = pos({ 0: 'xxxx*...........' }));
  eq(isForbidden(bd, star), false, '贴边恰好五连合法');
  ({ bd, star } = pos({ 7: '.........xxxxx*.' }));
  eq(isForbidden(bd, star), true, '右缘长连禁手');
  ({ bd, star } = pos({ 7: '..........xxxx*.' }));
  eq(isForbidden(bd, star), false, '右缘恰好五连合法');

  /* 白方与无禁模式不存在禁手:isForbidden 只对黑定义,合法点不被过滤 */
  ({ bd, star } = pos({
    6: '.......o........',
    7: '.....oo*........',
    8: '.......o........',
  }));
  eq(isForbidden(bd, star), false, 'isForbidden 只判黑棋(白棋同形不禁)');
  const freeMoves = legalMoves(bd, BLACK, FREE);
  ok(freeMoves.includes(star), '无禁模式:同样的点合法');
});

/* ==================== 3. 禁手对拍(引擎 vs 暴力判定器) ==================== */
section('forbidden-fuzz', () => {
  /* 固定种子伪随机,失败可复现 */
  let seed = 0x2f6e2b1;
  const rnd = () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x80000000;
  };

  let checked = 0, mismatches = 0;
  for (let t = 0; t < 60; t++) {
    const bd = new Int8Array(SIZE);
    const n = 3 + Math.floor(rnd() * 12);          // 3~14 颗子
    for (let i = 0; i < n; i++) {
      const c = Math.floor(rnd() * SIZE);
      bd[c] = i % 2 === 0 ? 1 : 2;                  // 尽量黑白交替,像真实局面
    }
    /* 保证没有现成五连(终局局面没有禁手判定的意义) */
    let five = false;
    for (let c = 0; c < SIZE; c++) if (bd[c] && bruteMadeFive(bd, c, FREE)) { five = true; break; }
    if (five) continue;
    syncPosition(bd);
    for (let c = 0; c < SIZE; c++) {
      if (bd[c] !== 0) continue;
      const eng = isForbidden(bd, c);
      const bru = bruteForbidden(bd, c);
      checked++;
      if (eng !== bru) {
        mismatches++;
        if (mismatches <= 3) {
          console.log(`  ✗ 禁手对拍不一致 cell=${c} 引擎=${eng} 暴力=${bru}`);
          for (let r = 0; r < 15; r++) {
            let row = '';
            for (let cc = 0; cc < 15; cc++) {
              const v = bd[r * 15 + cc];
              row += r * 15 + cc === c ? '*' : v === 1 ? 'x' : v === 2 ? 'o' : '.';
            }
            console.log(`    ${row}`);
          }
        }
      }
    }
  }
  ok(checked > 3000, `对拍样本充足(实际 ${checked} 点)`);
  eq(mismatches, 0, '引擎禁手与暴力判定器逐点一致');
});

/* ==================== 4. 增量状态一致性 ==================== */
section('state', () => {
  let seed = 0x51f2a3b;
  const rnd = () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x80000000;
  };
  for (let t = 0; t < 40; t++) {
    const bd = newBoard();
    const mvs = [];
    for (let i = 0; i < 20; i++) {
      const empties = [];
      for (let c = 0; c < SIZE; c++) if (bd[c] === 0) empties.push(c);
      const c = empties[Math.floor(rnd() * empties.length)];
      make(bd, c, sideToMove(bd));
      mvs.push(c);
    }
    const before = debugState();
    /* make + unmake 一对必须完整还原所有增量状态 */
    let probe = Math.floor(rnd() * SIZE);
    while (bd[probe] !== 0) { probe = (probe + 1) % SIZE; }
    make(bd, probe, sideToMove(bd));
    unmake(bd, probe, (sideToMove(bd) + 1) % 2);
    const after = debugState();
    ok(JSON.stringify(before) === JSON.stringify(after), `make/unmake 完整还原 (t=${t})`);

    /* 重放同样序列到新棋盘,所有派生状态必须一致(增量 == 全量重算) */
    const bd2 = newBoard();
    for (const c of mvs) make(bd2, c, sideToMove(bd2));
    ok(JSON.stringify(debugState()) === JSON.stringify(after), `重放一致 (t=${t})`);

    /* 撤销全部:回到空盘状态(撤的是「刚落子的一方」= sideToMove ^ 1) */
    for (let i = mvs.length - 1; i >= 0; i--) unmake(bd2, mvs[i], sideToMove(bd2) ^ 1);
    const emptyState = debugState();
    ok(emptyState.stones === 0 && emptyState.evalScore === 0 && emptyState.hk1 === 0,
      `撤回到空盘归零 (t=${t})`);
  }
});

/* ==================== 5. 合法着法与重演 ==================== */
section('moves', () => {
  const bd = newBoard();
  eq(legalMoves(bd, BLACK, RENJU).length, 9, '空盘候选点 = 天元附近 9 点');
  eq(sideToMove(bd), BLACK, '空盘轮黑');

  /* 有禁:禁手点被过滤出合法着法 */
  const { bd: bd2, star } = pos({
    6: '.....x.x........',
    7: '....x.x*........',
    8: '.....x.x........',
  });
  ok(!legalMoves(bd2, BLACK, RENJU).includes(star), '双三点不在黑方合法着法里');
  ok(legalMoves(bd2, BLACK, FREE).includes(star), '无禁模式下同一点合法');
  ok(legalMoves(bd2, WHITE, RENJU).includes(star), '白方不受禁手限制');
  const fps = forbiddenPoints(bd2);
  eq(fps.length, 2, '该局面有两个禁手点(测试点 + 横竖二拍的交点)');
  ok(fps.includes(star) && fps.includes(cell(7, 5)), `禁手点集合正确: ${fps}`);

  /* 重演:合法序列 → 返回行棋方;序列里混入禁手 → 拒绝 */
  const seq = [112, 97, 113, 98, 114, 99, 115, 100, 116];   // 末手黑五连
  eq(replayMoves(newBoard(), seq, FREE), -2, '末手成五返回 -2');
  eq(replayMoves(newBoard(), [112, 112], FREE), -1, '重复落点拒绝');
  eq(replayMoves(newBoard(), seq.slice(0, 4), FREE), BLACK, '前缀重演正常(4 手轮黑)');
  eq(replayMoves(newBoard(), seq.slice(0, 7), FREE), WHITE, '前缀重演正常(7 手轮白)');
  eq(replayMoves(newBoard(), [...seq, 60], FREE), -1, '成五后继续走拒绝');
});

/* ==================== 6. 搜索行为 ==================== */
section('search', () => {
  /* 一手成五:直接找到杀 */
  let bd = newBoard();
  for (const c of [112, 97, 113, 98, 114, 99, 115]) make(bd, c, sideToMove(bd));
  let r = searchBest(bd, BLACK, { mode: FREE, depth: 4, nodes: 100000, ms: 2000 });
  ok([111, 116].includes(r.move), `活四必变成五(实际 ${r.move})`);
  eq(r.mate, true, '报告杀棋');
  ok(r.score > MATE - 200, '杀棋分');

  /* 挡对方的冲四:白只有一个成五点,黑必须堵 */
  ({ bd } = pos({
    6: '.oooox..........',
    8: '......xxx.......',
  }));
  r = searchBest(bd, BLACK, { mode: FREE, depth: 4, nodes: 100000, ms: 2000 });
  eq(r.move, cell(6, 0), `黑堵白的唯一成五点(实际 ${r.move})`);

  /* 有禁模式:双三点不作为根着法返回 */
  const { bd: bd2, star } = pos({
    6: '.....x.x........',
    7: '....x.x*........',
    8: '.....x.x........',
  });
  r = searchBest(bd2, BLACK, { mode: RENJU, depth: 2, nodes: 20000, ms: 1000 });
  ok(r.move !== star, '有禁模式不返回禁手点');

  /* 返回的着法永远合法 */
  r = searchBest(bd2, BLACK, { mode: RENJU, depth: 4, nodes: 50000, ms: 2000 });
  ok(legalMoves(bd2, BLACK, RENJU).includes(r.move), '搜索结果在合法着法表内');

  /* 空盘第一手:天元 */
  r = searchBest(newBoard(), BLACK, { mode: FREE, depth: 2, nodes: 20000, ms: 1000 });
  eq(r.move, 112, '空盘第一手天元');

  /* 唯一候选直接返回 */
  const bd3 = newBoard();
  for (let i = 0; i < SIZE - 1; i++) {
    if (i === 112) continue;
    bd3[i] = i % 2 === 0 ? 2 : 1;               // 填满除天元外全部(任意子,不判胜负细节)
  }
  syncPosition(bd3);
  bd3[112] = 0;
  r = searchBest(bd3, BLACK, { mode: FREE, depth: 2, nodes: 20000, ms: 1000 });
  eq(r.move, 112, '只剩一个空点时直接返回');
});

/* ==================== 7. 随机对局(双模式全流程) ==================== */
section('games', () => {
  let seed = 0x19760918;
  const rnd = () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x80000000;
  };

  for (let t = 0; t < 30; t++) {
    const mode = t % 2 === 0 ? FREE : RENJU;
    const bd = newBoard();
    const seq = [];
    let winner = -1, winCell = -1, full = false;
    for (let i = 0; i < SIZE; i++) {
      const side = sideToMove(bd);
      const mvs = legalMoves(bd, side, mode);
      if (mvs.length === 0) { full = stoneCount() >= SIZE; break; }
      const c = mvs[Math.floor(rnd() * mvs.length)];
      make(bd, c, side);
      seq.push(c);
      if (madeFive(bd, c, mode)) { winner = side; winCell = c; break; }
    }
    /* 胜负与暴力一致 */
    if (winner >= 0) {
      eq(bruteMadeFive(bd, winCell, mode), true, `对局 ${t}:胜手暴力确认`);
      ok(checkWin(bd, winCell, mode)?.length >= 5, `对局 ${t}:胜连线正确`);
    }
    /* 序列可被 Worker 重演 */
    const bd2 = newBoard();
    const rr = replayMoves(bd2, seq, mode);
    ok(rr === -2 || rr >= 0, `对局 ${t}:重演接受整个序列(返回 ${rr})`);
    ok(JSON.stringify(Array.from(bd2)) === JSON.stringify(Array.from(bd)), `对局 ${t}:重演棋盘一致`);
    /* 有禁模式黑棋从没走过禁手点(legalMoves 已保证,这里对拍暴力) */
    if (mode === RENJU) {
      const bd3 = newBoard();
      let okSeq = true;
      for (const c of seq) {
        if (sideToMove(bd3) === BLACK && bruteForbidden(Int8Array.from(bd3), c)) { okSeq = false; break; }
        make(bd3, c, sideToMove(bd3));
        if (madeFive(bd3, c, mode)) break;
      }
      ok(okSeq, `对局 ${t}:黑棋序列无禁手(暴力确认)`);
    }
    void full;
  }
});

/* ==================== 8. 规则计数(perft 风格) ==================== */
section('perft', () => {
  /* 空盘候选 9 点(引擎的候选口径),浅层计数作为规则回归哨兵 */
  const bd = newBoard();
  eq(perft(bd, BLACK, FREE, 1), 9, '空盘深度 1 = 9 候选');
  const n2 = perft(bd, BLACK, FREE, 2);
  eq(n2, 9 * 24, '空盘深度 2 = 9×24(单子后候选为其 24 邻域,无五连终止)');

  /* 有禁模式:候选被禁手过滤后计数下降 */
  const { bd: bd2 } = pos({
    6: '.....x.x........',
    7: '....x.x.........',
    8: '.....x.x........',
  });
  const freeN = legalMoves(bd2, BLACK, FREE).length;
  const renjuN = legalMoves(bd2, BLACK, RENJU).length;
  ok(renjuN < freeN, `禁手过滤生效(有禁 ${renjuN} < 无禁 ${freeN})`);
});

/* ==================== 9. 难度档与评估 ==================== */
section('levels', () => {
  eq(LEVELS.length, 4, '四档难度');
  ok(LEVELS.every((lv) => lv.depth > 0 && lv.nodes > 0), '档位参数齐全');

  const bd = newBoard();
  eq(evaluate(bd, BLACK), 8, '空盘评估 = tempo(窗分为 0)');
  eq(evaluate(bd, WHITE), 8, 'tempo 对任意查询方都加(终局判定与查表方无关)');
  for (const c of [112, 97]) make(bd, c, sideToMove(bd));
  ok(evaluate(bd, BLACK) > 0, '黑多一子居中,黑视角为正');
  ok(evaluate(bd, BLACK) === -evaluate(bd, WHITE) + 16, '评估满足零和(+2×tempo)');
});

/* ---------- 运行 ---------- */
const only = process.argv.slice(2).filter((a) => a !== '--list');
for (const { name, fn } of sections) {
  if (only.length && !only.includes(name)) continue;
  const before = pass;
  console.log(`\n== ${name} ==`);
  fn();
  const n = pass - before;
  console.log(`   (${n} 项通过)`);
}
console.log(`\n${fail ? '✗ ' + fail + ' 项失败' : '✓ 全部通过'}(共 ${pass + fail} 项)`);
process.exit(fail ? 1 : 0);
