/* ============================================================
 * AetherRenju —— 五子棋 / 连珠引擎(规则 + 评估 + 搜索)
 *
 * 棋盘:15×15 = 225 点,idx = 行×15 + 列;黑先白后。
 * 棋子:0 空 / 1 黑 / 2 白(方:0 黑 BLACK / 1 白 WHITE)。
 * 「着法」就是一个点 idx —— 五子棋没有吃子升变,UI 与 Worker 之间
 *   只传这一种编码,不搞第二套。
 *
 * 规则(两种模式):
 *   无禁(FREE,自由五子棋):任意一方先连成 ≥5 子即胜(长连也算)。
 *   有禁(RENJU,连珠):黑方恰好五连才胜;黑方走「长连 / 双四 / 双活三」
 *     直接判负(所以引擎把它们当作非法着法,不生成、不搜索);
 *     白方无任何限制,五连或长连都胜。
 *   禁手判定按 RIF 规则递归展开(见 isForbidden 顶部注释)。
 *
 * 搜索:negamax + alpha-beta + 迭代加深 + 置换表(Zobrist 双 32 位校验)
 *      + 候选点增益排序 + killer / history + 强迫步静态搜索(四/五威胁)。
 * 评估:五元窗计数(4 个方向 572 个窗),随走子增量维护,叶节点 O(1) 取值。
 *      参数是手调初值,没有自对弈拟合 —— 见 docs/ROADMAP.md。
 *
 * 对外入口:
 *   newBoard() / replayMoves() / legalMoves() / forbiddenPoints() / checkWin()
 *   isForbidden()                —— UI 侧禁手标记
 *   searchBest(bd, side, opt)    —— Worker 侧搜索
 *   LEVELS                       —— 难度档(节点预算为主、墙上时间为辅)
 *
 * 本文件不碰 DOM、不 import 任何库 —— 浏览器、Worker、Node 通用。
 * ============================================================ */

export const BLACK = 0, WHITE = 1;
export const FREE = 0, RENJU = 1;            // 规则模式:无禁 / 有禁(连珠)

export const N = 15, SIZE = 225;             // 棋盘边长与点数
export const EMPTY = 0;                      // 格值:0 空 / 1 黑 / 2 白

export const MATE = 30000;
const INF = 1 << 28;
const MAXPLY = 96;

/* ==================== 增量维护的全局状态 ====================
 * 与 AetherXiangqi 同一套路:棋盘外的派生数据全是模块级 TypedArray,
 * make/unmake 对称更新,搜索过程零分配。
 * 一个进程同一时刻只有一张活跃棋盘(Node 测试 / Worker / UI 都满足)。 */
let HK1 = 0, HK2 = 0;                        // Zobrist 键(只含子力,行棋方另异或)
let stones = 0;                              // 盘上子数(奇偶 = 轮到谁)
let evalScore = 0;                           // 全体五元窗得分之和(黑正白负)
let gameMode = RENJU;                        // 当前规则模式(searchBest 时设定)

/* 邻居表:切比雪夫距离 ≤2 的 24 个邻点,-1 表示盘外。
 * 候选点生成靠它:NEI[q] > 0 的空点才是「有意义的落点」。 */
const NEI = new Int16Array(SIZE);
const NBL = new Int16Array(SIZE * 24);
{
  let n = 0;
  const tmp = new Int16Array(24);
  for (let i = 0; i < SIZE; i++) {
    const r = (i / N) | 0, c = i % N;
    tmp.fill(-1);
    let m = 0;
    for (let dr = -2; dr <= 2; dr++) {
      for (let dc = -2; dc <= 2; dc++) {
        if (!dr && !dc) continue;
        const rr = r + dr, cc = c + dc;
        if (rr >= 0 && rr < N && cc >= 0 && cc < N) tmp[m++] = rr * N + cc;
      }
    }
    for (let k = 0; k < 24; k++) NBL[n++] = tmp[k];
  }
}

/* ==================== 五元窗(评估的骨架) ====================
 * 4 个方向上所有长度为 5 的连续线段,共 572 个窗。
 * 每个窗按「黑子数 b / 白子数 w」计分:混色 = 0 分(废窗),
 * 纯黑 = +W[b],纯白 = −W[w]。全盘分 = Σ窗分,随 make/unmake 增量维护。
 * CNT 是「处于每种 (b,w) 状态的窗口数」,给 qsearch 做 O(1) 快筛。 */
const DIRS = 4;
const DR = [0, 1, 1, 1], DC = [1, 0, 1, -1];
const STRIDE = [1, N, N + 1, N - 1];         // 各方向相邻点的 idx 步长

const NW = 165 * 2 + 121 * 2;                // 572
const WCELLS = new Int16Array(NW * 5);       // 窗 → 5 个点
const WB = new Int8Array(NW), WW = new Int8Array(NW);   // 窗内黑/白子数
const CNT = new Int32Array(36);              // (b,w) → 窗口数
const CWCOUNT = new Uint8Array(SIZE);
const CW = new Int32Array(SIZE * 20);        // 点 → 所属窗(每方向至多 5 个)
{
  let w = 0;
  for (let d = 0; d < DIRS; d++) {
    const dr = DR[d], dc = DC[d];
    for (let r = 0; r < N; r++) {
      for (let c = 0; c < N; c++) {
        const er = r + dr * 4, ec = c + dc * 4;          // 窗尾
        if (er < 0 || er >= N || ec < 0 || ec >= N) continue;
        for (let k = 0; k < 5; k++) {
          const cell = (r + dr * k) * N + (c + dc * k);
          WCELLS[w * 5 + k] = cell;
          CW[cell * 20 + CWCOUNT[cell]++] = w;
        }
        w++;
      }
    }
  }
}

/* 窗分权重(百分制手调初值):1 子起步、成四跳档、五连封顶。
 * 活四 ≈ 2×W4 + 若干 W3,冲四 ≈ W4 —— 活四明显高于冲四,
 * 活三 ≈ 3×W3,眠三只有 1 个 W3 —— 层次是拉开的。 */
const W = [0, 4, 36, 320, 2800, 1200000];
const WSC = new Int32Array(36);              // WSC[b*6+w]
for (let b = 0; b <= 5; b++) {
  for (let o = 0; o <= 5; o++) {
    WSC[b * 6 + o] = (b > 0 && o > 0) ? 0 : (b > 0 ? W[b] : (o > 0 ? -W[o] : 0));
  }
}

const TEMPO = 8;                             // 先手微加成

/* ==================== Zobrist ==================== */

const Z1 = new Int32Array(2 * SIZE), Z2 = new Int32Array(2 * SIZE);
{
  let s = 0x51f2a3b7;
  const rnd = () => {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return (t ^ (t >>> 14)) | 0;
  };
  for (let i = 0; i < Z1.length; i++) { Z1[i] = rnd(); Z2[i] = rnd(); }
}
const ZS1 = rndSideKey(0x9e3779b9), ZS2 = rndSideKey(0x85ebca6b);
function rndSideKey(seed) {
  let s = seed;
  const rnd = () => {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return (t ^ (t >>> 14)) | 0;
  };
  for (let i = 0; i < 8; i++) rnd();
  return rnd();
}

/* ==================== 局面构建 ==================== */

/** 空盘(五子棋从空盘开始,黑先) */
export function newBoard() {
  const bd = new Int8Array(SIZE);
  syncPosition(bd);
  return bd;
}

/** 重算全部派生状态(键 / 邻居数 / 窗计数 / 全盘分);摆棋后必须调用 */
export function syncPosition(bd) {
  HK1 = 0; HK2 = 0; stones = 0;
  NEI.fill(0);
  for (let i = 0; i < SIZE; i++) {
    const p = bd[i];
    if (!p) continue;
    stones++;
    HK1 ^= Z1[(p - 1) * SIZE + i]; HK2 ^= Z2[(p - 1) * SIZE + i];
    const base = i * 24;
    for (let k = 0; k < 24; k++) { const q = NBL[base + k]; if (q >= 0) NEI[q]++; }
  }
  evalScore = 0; CNT.fill(0);
  for (let w = 0; w < NW; w++) {
    let b = 0, o = 0;
    for (let k = 0; k < 5; k++) {
      const p = bd[WCELLS[w * 5 + k]];
      if (p === 1) b++; else if (p === 2) o++;
    }
    WB[w] = b; WW[w] = o;
    evalScore += WSC[b * 6 + o]; CNT[b * 6 + o]++;
  }
}

/** stones 的奇偶即行棋方:黑先 */
export function sideToMove(bd) { void bd; return stones % 2 === 0 ? BLACK : WHITE; }
export function stoneCount() { return stones; }

/** 从空盘按落点序列重演(Worker 用它还原 UI 的棋盘)。
 *  序列不合法(占点 / 轮次 / 禁手 / 终局后还有棋)返回 -1。 */
export function replayMoves(bd, moves, mode = RENJU) {
  let s = BLACK;
  for (let i = 0; i < moves.length; i++) {
    const cell = moves[i];
    if (cell < 0 || cell >= SIZE || bd[cell] !== 0) return -1;
    if (mode === RENJU && s === BLACK && isForbidden(bd, cell)) return -1;
    make(bd, cell, s);
    if (madeFive(bd, cell, mode)) return i === moves.length - 1 ? -2 : -1;  // -2 = 到手为止
    s ^= 1;
  }
  return s;
}

/* ==================== 走子 ==================== */

/** 落子:增量维护键 / 邻居数 / 窗计数 / 全盘分 */
export function make(bd, cell, side) {
  const p = side + 1;
  bd[cell] = p; stones++;
  HK1 ^= Z1[side * SIZE + cell]; HK2 ^= Z2[side * SIZE + cell];
  const nb = cell * 24;
  for (let k = 0; k < 24; k++) { const q = NBL[nb + k]; if (q >= 0) NEI[q]++; }
  const cw = cell * 20, cn = CWCOUNT[cell];
  for (let k = 0; k < cn; k++) {
    const w = CW[cw + k];
    const b = WB[w], o = WW[w];
    evalScore -= WSC[b * 6 + o]; CNT[b * 6 + o]--;
    if (side === BLACK) { WB[w] = b + 1; evalScore += WSC[(b + 1) * 6 + o]; CNT[(b + 1) * 6 + o]++; }
    else { WW[w] = o + 1; evalScore += WSC[b * 6 + o + 1]; CNT[b * 6 + o + 1]++; }
  }
}

/** 撤销:异或与计数都是自逆/对称的,和 make 完全镜像 */
export function unmake(bd, cell, side) {
  const p = side + 1;
  bd[cell] = 0; stones--;
  HK1 ^= Z1[side * SIZE + cell]; HK2 ^= Z2[side * SIZE + cell];
  const nb = cell * 24;
  for (let k = 0; k < 24; k++) { const q = NBL[nb + k]; if (q >= 0) NEI[q]--; }
  const cw = cell * 20, cn = CWCOUNT[cell];
  for (let k = 0; k < cn; k++) {
    const w = CW[cw + k];
    const b = WB[w], o = WW[w];
    evalScore -= WSC[b * 6 + o]; CNT[b * 6 + o]--;
    if (side === BLACK) { WB[w] = b - 1; evalScore += WSC[(b - 1) * 6 + o]; CNT[(b - 1) * 6 + o]++; }
    else { WW[w] = o - 1; evalScore += WSC[b * 6 + o - 1]; CNT[b * 6 + o - 1]++; }
  }
}

/* ==================== 胜负判定 ==================== */

/** cell 落下后沿 dir 的连长(含自身) */
function runLen(bd, cell, p, d) {
  const r0 = (cell / N) | 0, c0 = cell % N, dr = DR[d], dc = DC[d];
  let n = 1;
  for (let s = -1; s <= 1; s += 2) {
    let r = r0 + dr * s, c = c0 + dc * s;
    while (r >= 0 && r < N && c >= 0 && c < N && bd[r * N + c] === p) {
      n++; r += dr * s; c += dc * s;
    }
  }
  return n;
}

/** cell(已落)是否成五。有禁黑方必须恰好五连;其余(无禁双方 / 有禁白方)≥5 即胜 */
export function madeFive(bd, cell, mode = gameMode) {
  const p = bd[cell];
  if (!p) return false;
  const side = p - 1;
  const exact = mode === RENJU && side === BLACK;
  for (let d = 0; d < DIRS; d++) {
    const L = runLen(bd, cell, p, d);
    if (exact ? L === 5 : L >= 5) return true;
  }
  return false;
}

/** cell(已落)的获胜连线(整段连子,长连时 >5 个),没赢返回 null。UI 画高亮用 */
export function checkWin(bd, cell, mode = gameMode) {
  const p = bd[cell];
  if (!p) return null;
  const side = p - 1;
  const exact = mode === RENJU && side === BLACK;
  for (let d = 0; d < DIRS; d++) {
    const r0 = (cell / N) | 0, c0 = cell % N, dr = DR[d], dc = DC[d];
    const line = [cell];
    for (let s = -1; s <= 1; s += 2) {
      let r = r0 + dr * s, c = c0 + dc * s;
      while (r >= 0 && r < N && c >= 0 && c < N && bd[r * N + c] === p) {
        if (s === -1) line.unshift(r * N + c); else line.push(r * N + c);
        r += dr * s; c += dc * s;
      }
    }
    const L = line.length;
    if (exact ? L === 5 : L >= 5) return line;
  }
  return null;
}

/* ==================== 禁手(有禁模式,黑方) ====================
 * RIF 规则的递归实现。记落点为 x(先假设黑下在 x):
 *
 *   五连    恰好五连 → 直接获胜,禁手全部豁免(先判,优先级最高)。
 *   长连    ≥6 连 → 禁手。
 *   四      补一子能成「恰好五」的形状;同一方向上以「补五点的成五窗」
 *           的子集合去重(两端的两个补五点若凑成同一组 4 子 = 活四,算一个四)。
 *   活四    恰好 4 连,两端都是空点,且两端再外侧无黑子(否则补了是长连)。
 *   三      存在一个空点 p,黑补在 p 后能在该方向形成活四(活四必含 x 与 p);
 *           且补 p 这手棋本身合法 —— 不成五(三的定义排除同时成五)、
 *           不成禁手(递归判定)。这就是「假活三不算三」的官方口径。
 *   双四    一步造成 ≥2 个四(允许两个四在同一方向:●●●_x_●●●)。
 *   双三    一步造成 ≥2 个三(逐方向判定;同一方向至多一个三,
 *           因为一条线上过同一 x 的两个活四会因端点条件互相顶死)。
 *
 * 全部检查都在以 x 为中心的 ±5 线段缓冲上进行 —— 五连窗、活四的
 * 端点与外沿最远触及 x±5,缓冲外用「白子」填(盘外挡子,与盘边同义)。 */

const LBUF = new Int8Array(11);              // ±5 线段缓冲,下标 5 = 中心
const PSNAPS = Array.from({ length: 12 }, () => new Int8Array(11));  // p 候选快照,按递归深度分槽(内层递归会各自覆写)
const FSETS = new Int8Array(5 * 4);          // fourCountDir 的成五窗子集合去重区
const CURSET = new Int8Array(4);

function fillLine(bd, cell, d) {
  const r0 = (cell / N) | 0, c0 = cell % N, dr = DR[d], dc = DC[d];
  for (let k = -5; k <= 5; k++) {
    const rr = r0 + dr * k, cc = c0 + dc * k;
    LBUF[k + 5] = (rr >= 0 && rr < N && cc >= 0 && cc < N) ? bd[rr * N + cc] : 2;
  }
}

/** 中心落黑后,dir 上「经过中心的四」的个数(0/1/2,按成五窗的子集合去重) */
function fourCountDir(bd, cell, d) {
  fillLine(bd, cell, d);
  let sets = 0;
  for (let k = 1; k <= 5; k++) {             // 含中心(下标 5)的窗:起点 1..5
    let b = 0, q = -1, dead = false;
    for (let i = k; i <= k + 4; i++) {
      const v = LBUF[i];
      if (v === 2) { dead = true; break; }
      if (v === 1) b++; else q = i;
    }
    if (dead || b !== 4 || q < 0) continue;  // 恰好 4 黑 1 空:补 q 成五
    if (LBUF[k - 1] === 1 || LBUF[k + 5] === 1) continue;   // 外沿有黑 → 补了是长连
    /* 成五窗的 4 个子(去掉 q)作为这个四的指纹,去重(活四两个补五点算一个四) */
    let m = 0;
    for (let i = k; i <= k + 4; i++) if (i !== q) CURSET[m++] = i;
    let same = false;
    for (let s = 0; s < sets; s++) {
      let hit = true;
      for (let j = 0; j < 4; j++) if (FSETS[s * 4 + j] !== CURSET[j]) { hit = false; break; }
      if (hit) { same = true; break; }
    }
    if (same) continue;
    for (let j = 0; j < 4; j++) FSETS[sets * 4 + j] = CURSET[j];
    sets++;
  }
  return sets;
}

/** dir 上是否存在「真活三」:存在空点 p,补 p 后出现过中心且过 p 的活四,
 *  p 本身不成五(≤4 连)且不是禁手(递归)。 */
function hasRealThree(bd, cell, d, depth) {
  fillLine(bd, cell, d);
  /* 候选点先快照:下面的递归(isForbidden → fourCountDir)会重填 LBUF,
   * 不快照的话后续迭代会把已占点误判成空点,直接破坏棋盘状态 */
  PSNAPS[depth].set(LBUF);
  const stride = STRIDE[d];
  for (let pi = 2; pi <= 8; pi++) {          // p 只可能在中心 ±3(四连要同时装下两点)
    if (pi === 5 || PSNAPS[depth][pi] !== 0) continue;
    const pcell = cell + (pi - 5) * stride;
    make(bd, pcell, BLACK);
    fillLine(bd, cell, d);
    let ok = false;
    /* 活四 = 恰好 4 连,含中心(5)与 p(pi),两端空、两端外侧无黑 */
    for (let s = 2; s <= 5; s++) {           // 过中心的 4 连起点
      if (pi < s || pi > s + 3) continue;
      let all = true;
      for (let i = s; i <= s + 3; i++) if (LBUF[i] !== 1) { all = false; break; }
      if (!all) continue;
      if (LBUF[s - 1] !== 0 || LBUF[s + 4] !== 0) continue;
      if (LBUF[s - 2] === 1 || LBUF[s + 5] === 1) continue;
      ok = true; break;
    }
    if (ok) {
      /* 补 p 不得同时成五/长连(RIF 三的定义) */
      for (let dd = 0; dd < DIRS && ok; dd++) {
        if (runLen(bd, pcell, 1, dd) >= 5) ok = false;
      }
    }
    unmake(bd, pcell, BLACK);
    /* isForbidden 要求空点:先撤掉 p 再判(它自己会重新落子) */
    if (ok) ok = !isForbidden(bd, pcell, depth + 1);
    if (ok) return true;
  }
  return false;
}

/* 禁手判定缓存:键 = 盘面键 ⊕ 该点黑子 Zobrist(纯几何性质,与轮谁无关) */
const FB_BITS = 14, FB_SIZE = 1 << FB_BITS, FB_MASK = FB_SIZE - 1;
const fbK1 = new Int32Array(FB_SIZE), fbK2 = new Int32Array(FB_SIZE);
const fbV = new Int8Array(FB_SIZE);

/**
 * 有禁模式下,黑棋下在空点 cell 是否禁手(长连 / 双四 / 双活三)。
 * 五连优先:同时成五则不是禁手(黑直接获胜)。
 * depth 只作递归保险(>8 视为禁手,实际局面到不了这个深度)。
 */
export function isForbidden(bd, cell, depth = 0) {
  if (depth > 8) return true;
  const k1 = HK1 ^ Z1[cell], k2 = HK2 ^ Z2[cell];
  const fbi = k1 & FB_MASK;
  if (fbK1[fbi] === k1 && fbK2[fbi] === k2) return fbV[fbi] === 1;

  make(bd, cell, BLACK);
  let five = false, over = false;
  for (let d = 0; d < DIRS; d++) {
    const L = runLen(bd, cell, 1, d);
    if (L === 5) { five = true; break; }
    if (L >= 6) over = true;
  }
  let forbidden;
  if (five) forbidden = false;               // 成五豁免一切禁手
  else if (over) forbidden = true;           // 长连禁手
  else {
    let fours = 0;
    for (let d = 0; d < DIRS; d++) {
      fours += fourCountDir(bd, cell, d);
      if (fours >= 2) break;
    }
    if (fours >= 2) forbidden = true;        // 双四
    else {
      let threes = 0;
      for (let d = 0; d < DIRS; d++) {
        if (hasRealThree(bd, cell, d, depth)) { threes++; if (threes >= 2) break; }
      }
      forbidden = threes >= 2;               // 双活三
    }
  }
  unmake(bd, cell, BLACK);

  fbK1[fbi] = k1; fbK2[fbi] = k2; fbV[fbi] = forbidden ? 1 : 0;
  return forbidden;
}

/** 有禁模式下黑方的全部禁手点(UI 画 × 标记用) */
export function forbiddenPoints(bd) {
  const out = [];
  for (let i = 0; i < SIZE; i++) {
    if (bd[i] === 0 && NEI[i] > 0 && isForbidden(bd, i)) out.push(i);
  }
  return out;
}

/* ==================== 候选点与合法着法 ==================== */

/** 候选点写入 MB[base..],返回个数:有邻居的空点;空盘给天元附近 9 点。
 *  邻居区一颗空点都没有(孤岛病态局面)时回退为全盘空点 —— 合法性
 *  判定在任何局面都成立,远点也永远不可能是禁手(禁手要有子参与)。 */
function genCandidates(bd, side, base) {
  let n = 0;
  if (stones === 0) {
    /* 空盘:天元附近 9 点,天元优先(评估并列时第一候选胜出 → AI 执黑先行落天元) */
    for (const [r, c] of [[7, 7], [6, 6], [6, 7], [6, 8], [7, 6], [7, 8], [8, 6], [8, 7], [8, 8]]) {
      const cell = r * N + c;
      MB[base + n] = cell;
      MS[base + n] = 9 - Math.max(Math.abs(r - 7), Math.abs(c - 7));
      n++;
    }
    return n;
  }
  for (let i = 0; i < SIZE; i++) {
    if (bd[i] !== 0 || NEI[i] === 0) continue;
    MB[base + n] = i;
    MS[base + n] = pointScore(i, side);
    n++;
  }
  if (n === 0) {
    for (let i = 0; i < SIZE; i++) {
      if (bd[i] !== 0) continue;
      MB[base + n] = i;
      MS[base + n] = 0;
      n++;
    }
  }
  return n;
}

/** 落点增益 = 若此手落下全盘分的变化量。评估是零和对称的,
 *  「挡住对方的四」与「自己成四」都会自然变成大正分,一套打分通吃攻防。 */
function pointScore(cell, side) {
  const cw = cell * 20, cn = CWCOUNT[cell];
  let s = 0;
  for (let k = 0; k < cn; k++) {
    const w = CW[cw + k];
    const b = WB[w], o = WW[w];
    if (b > 0 && o > 0) continue;            // 废窗,落了也 0 变化
    const cur = WSC[b * 6 + o];
    s += side === BLACK ? WSC[(b + 1) * 6 + o] - cur : WSC[b * 6 + o + 1] - cur;
  }
  return s;
}

/** 合法落点数组:候选点里剔掉黑方禁手(有禁模式) */
export function legalMoves(bd, side, mode = gameMode) {
  const n = genCandidates(bd, side, 0);
  const out = [];
  for (let i = 0; i < n; i++) {
    const cell = MB[i];
    if (mode === RENJU && side === BLACK && isForbidden(bd, cell)) continue;
    out.push(cell);
  }
  return out;
}

/** 还有没有合法落点(满盘 / 有禁黑被禁手彻底封死 → 无) */
export function hasLegalMove(bd, side, mode = gameMode) {
  return legalMoves(bd, side, mode).length > 0;
}

/* ==================== 评估 ==================== */

/** 静态评估(行棋方视角):全盘窗分 + tempo。窗分增量维护,这里 O(1) */
export function evaluate(bd, side) {
  void bd;
  return (side === BLACK ? evalScore : -evalScore) + TEMPO;
}

/* ==================== 置换表 ==================== */

const TT_BITS = 17, TT_SIZE = 1 << TT_BITS, TT_MASK = TT_SIZE - 1;
const ttK1 = new Int32Array(TT_SIZE), ttK2 = new Int32Array(TT_SIZE);
const ttMv = new Int32Array(TT_SIZE), ttSc = new Int32Array(TT_SIZE);
const ttDp = new Int8Array(TT_SIZE), ttFl = new Int8Array(TT_SIZE);

/** 每次搜索前清空:同一局面重复搜索结果一致(可复现) */
export function clearTT() {
  ttK1.fill(0); ttK2.fill(0); ttMv.fill(0); ttDp.fill(0); ttFl.fill(0);
  KILLER.fill(0);
  HIST.fill(0);
}

/* ==================== 搜索 ==================== */

const MOVE_CAP = SIZE + 1;                  // 单节点候选点上限(≤225)
const MB = new Int32Array(MOVE_CAP * (MAXPLY + 8));    // 候选点
const MS = new Int32Array(MOVE_CAP * (MAXPLY + 8));    // 平行的排序分
const KILLER = new Int32Array(MAXPLY * 2);
const HIST = new Int32Array(2 * SIZE);      // history:行棋方×点 → 累计加分

let nodes = 0;
let t0 = 0, nodeBudget = 0, msBudget = 0, aborted = false;
let qsCap = 14;                              // 静态搜索的绝对 ply 上限(强迫链 7 轮叫四,够战术深度)

function timeUp() {
  if (nodes > nodeBudget) { aborted = true; return true; }
  if ((nodes & 1023) === 0 && Date.now() - t0 > msBudget) { aborted = true; return true; }
  return false;
}

/** 主搜索(负极大 + alpha-beta)。禁手是懒判定:轮到尝试该点时才查,
 *  查过有小缓存,重复局面几乎零开销。 */
function search(bd, side, depth, alpha, beta, ply) {
  nodes++;
  if (timeUp()) return 0;
  if (stones >= SIZE) return 0;              // 满盘和棋
  if (depth <= 0) { qsCap = Math.max(qsCap, ply + 14); return qsearch(bd, side, alpha, beta, ply); }

  /* 置换表键:局面键再异或行棋方 */
  const k1 = HK1 ^ (side === WHITE ? ZS1 : 0);
  const idx = k1 & TT_MASK;
  let ttMove = -1;
  if (ttK1[idx] === k1 && ttK2[idx] === HK2) {
    ttMove = ttMv[idx];
    if (ttDp[idx] >= depth && ply > 0) {
      const sc = ttSc[idx], fl = ttFl[idx];
      if (fl === 0) return sc;
      if (fl === 2 && sc >= beta) return sc;
      if (fl === 3 && sc <= alpha) return sc;
    }
  }

  const base = ply * MOVE_CAP;
  const n = genCandidates(bd, side, base);
  if (n === 0) return 0;                     // 满盘和棋(其余局面总有候选点)
  /* 排序分:点增益为底,TT 点 / killer 置顶 */
  for (let i = 0; i < n; i++) {
    const mv = MB[base + i];
    if (mv === ttMove) MS[base + i] += 1 << 28;
    else if (mv === KILLER[ply * 2] || mv === KILLER[ply * 2 + 1]) MS[base + i] += 1 << 27;
  }

  let best = -INF, bestMove = -1, anyLegal = false, flag = 3;
  for (let i = 0; i < n; i++) {
    /* 选择排序取当前最大分:候选 ≤225,O(n²) 但截断早,比分配数组省 */
    let bi = i;
    for (let j = i + 1; j < n; j++) if (MS[base + j] > MS[base + bi]) bi = j;
    if (bi !== i) {
      const tmv = MB[base + i]; MB[base + i] = MB[base + bi]; MB[base + bi] = tmv;
      const tsc = MS[base + i]; MS[base + i] = MS[base + bi]; MS[base + bi] = tsc;
    }
    const cell = MB[base + i];
    if (gameMode === RENJU && side === BLACK && isForbidden(bd, cell)) continue;
    anyLegal = true;
    make(bd, cell, side);
    let sc;
    if (madeFive(bd, cell)) sc = MATE - ply - 1;   // 成五即胜,不再递归
    else sc = -search(bd, side ^ 1, depth - 1, -beta, -alpha, ply + 1);
    unmake(bd, cell, side);
    if (aborted) return 0;
    if (sc > best) {
      best = sc; bestMove = cell;
      if (sc > alpha) { alpha = sc; flag = 0; }
      if (alpha >= beta) {
        flag = 2;
        if (KILLER[ply * 2] !== cell) { KILLER[ply * 2 + 1] = KILLER[ply * 2]; KILLER[ply * 2] = cell; }
        HIST[side * SIZE + cell] += depth * depth;
        break;
      }
    }
  }

  if (!anyLegal) {
    /* 黑棋候选区全被禁手封死 = 必负(远处空点不会出现在中局的候选区,
     * 真走到那一步胜负早已分明,这里按规则口径给杀分) */
    if (gameMode === RENJU && side === BLACK) return -MATE + ply;
    return 0;
  }
  ttK1[idx] = k1; ttK2[idx] = HK2; ttMv[idx] = bestMove;
  ttSc[idx] = best; ttDp[idx] = Math.min(depth, 127); ttFl[idx] = flag;
  return best;
}

/** 静态搜索(VCF 式):严格分级 —— 有成五点必胜先走;对方叫五只许挡
 *  (不挡必输,自己成四来不及);否则才轮到自己成四叫杀。这样「成四 →
 *  被迫挡」严格交替,分支恒小,链深有上限,单叶开销可控。 */
function qsearch(bd, side, alpha, beta, ply) {
  nodes++;
  if (timeUp()) return 0;
  let best = evaluate(bd, side);
  if (best >= beta) return best;
  if (best > alpha) alpha = best;
  if (ply >= MAXPLY - 4 || ply >= qsCap) return best;

  /* O(1) 快筛 + 分级:my4 = 己方 4 子纯窗(成五点),op4 = 对方 4 子纯窗,
   * my3 = 己方 3 子纯窗(成四点) */
  const my4 = side === BLACK ? CNT[4 * 6] : CNT[4];
  const op4 = side === BLACK ? CNT[4] : CNT[4 * 6];
  const my3 = side === BLACK ? CNT[3 * 6] : CNT[3];
  if (my4 === 0 && op4 === 0 && my3 === 0) return best;

  const base = ply * MOVE_CAP;
  let n = 0;
  for (let i = 0; i < SIZE; i++) {
    if (bd[i] !== 0 || NEI[i] === 0) continue;
    const cw = i * 20, cn = CWCOUNT[i];
    let ok = false;
    for (let k = 0; k < cn && !ok; k++) {
      const w = CW[cw + k];
      const b = WB[w], o = WW[w];
      if (b > 0 && o > 0) continue;
      if (my4 > 0) ok = side === BLACK ? (b === 4 && o === 0) : (o === 4 && b === 0);
      else if (op4 > 0) ok = side === BLACK ? (o === 4 && b === 0) : (b === 4 && o === 0);
      else ok = side === BLACK ? (b === 3 && o === 0) : (o === 3 && b === 0);
    }
    if (!ok) continue;
    MB[base + n] = i;
    MS[base + n] = pointScore(i, side);
    n++;
  }
  if (n === 0) return best;

  for (let i = 0; i < n; i++) {
    let bi = i;
    for (let j = i + 1; j < n; j++) if (MS[base + j] > MS[base + bi]) bi = j;
    if (bi !== i) {
      const tmv = MB[base + i]; MB[base + i] = MB[base + bi]; MB[base + bi] = tmv;
      const tsc = MS[base + i]; MS[base + i] = MS[base + bi]; MS[base + bi] = tsc;
    }
    const cell = MB[base + i];
    if (gameMode === RENJU && side === BLACK && isForbidden(bd, cell)) continue;
    make(bd, cell, side);
    let sc;
    if (madeFive(bd, cell)) sc = MATE - ply - 1;
    else sc = -qsearch(bd, side ^ 1, -beta, -alpha, ply + 1);
    unmake(bd, cell, side);
    if (aborted) return 0;
    if (sc > best) {
      best = sc;
      if (sc > alpha) { alpha = sc; if (alpha >= beta) break; }
    }
  }
  return best;
}

/* ==================== 难度档 ==================== */
/* 节点预算为主(设备无关、可复现),墙上时间为兜底。
 * 实测(本机 Node 22,桌面级 CPU)安静局面约 20-29 万节点/s,
 * 深度按「预算内能稳定走完的层数」定 —— 见 bench/bench.mjs nps。
 * jitter:低难度在最优解附近随机挑一个,弱得可控。 */
export const LEVELS = [
  { id: 'easy', name: '初级', desc: '2 层搜索 + 随机化', depth: 2, nodes: 8000, ms: 400, jitter: 60 },
  { id: 'normal', name: '中级', desc: '4 层迭代加深', depth: 4, nodes: 40000, ms: 1200, jitter: 0 },
  { id: 'hard', name: '高级', desc: '6 层迭代加深', depth: 6, nodes: 200000, ms: 2500, jitter: 0 },
  { id: 'master', name: '大师', desc: '8 层迭代加深', depth: 8, nodes: 600000, ms: 5000, jitter: 0 },
];
export const DEFAULT_LEVEL = 2;

/**
 * 搜索最佳落点。
 * opt: { mode, depth, nodes, ms, jitter, onProgress({depth, move, score, nodes, ms}) }
 * 返回 { move, score, depth, nodes, ms, mate, draw, only }
 * move = -1 表示无点可下(有禁黑被禁手封死 = 负;满盘 = 和,draw=true)。
 */
export function searchBest(bd, side, opt = {}) {
  t0 = Date.now();
  nodeBudget = opt.nodes ?? 200000;
  msBudget = opt.ms ?? 3000;
  const maxDepth = opt.depth ?? 6;
  gameMode = opt.mode ?? RENJU;
  nodes = 0; aborted = false; qsCap = 14;
  clearTT();

  /* 根着法先做完整合法性过滤(禁手在根上就剔掉) */
  const cells = legalMoves(bd, side, gameMode);
  if (cells.length === 0) {
    const lost = gameMode === RENJU && side === BLACK && stones < SIZE;
    return { move: -1, score: lost ? -MATE : 0, depth: 0, nodes: 0, ms: 0,
             mate: lost, draw: !lost, only: false };
  }
  if (cells.length === 1) {
    return { move: cells[0], score: 0, depth: 1, nodes: 0, ms: Date.now() - t0, only: true, mate: false, draw: false };
  }
  const root = cells.map((cell) => ({ cell, sc: 0 }));

  let best = root[0].cell, bestScore = 0, doneDepth = 0;
  for (let d = 1; d <= maxDepth; d++) {
    let alpha = -INF;
    let iterBest = -1, iterScore = -INF;
    for (const item of root) {
      make(bd, item.cell, side);
      let sc;
      if (madeFive(bd, item.cell, gameMode)) sc = MATE - 1;
      else sc = -search(bd, side ^ 1, d - 1, -INF, -alpha, 1);
      unmake(bd, item.cell, side);
      if (aborted) break;
      item.sc = sc;
      if (sc > iterScore) { iterScore = sc; iterBest = item.cell; if (sc > alpha) alpha = sc; }
    }
    if (aborted) break;
    best = iterBest; bestScore = iterScore; doneDepth = d;
    root.sort((a, b) => b.sc - a.sc);        // 上一层分数当下一层的排序
    opt.onProgress?.({ depth: d, move: best, score: bestScore, nodes, ms: Date.now() - t0 });
    if (Math.abs(bestScore) > MATE - 200) break;   // 已算到杀棋
    if (Date.now() - t0 > msBudget) break;
  }

  /* 低难度:在最优解附近随机挑一个,弱得可控 */
  const jitter = opt.jitter ?? 0;
  if (jitter > 0 && bestScore > -MATE + 200) {
    const pool = root.filter((it) => it.sc >= bestScore - jitter);
    best = pool[(Math.random() * pool.length) | 0].cell;
  }

  return {
    move: best, score: bestScore, depth: doneDepth, nodes,
    ms: Date.now() - t0, mate: Math.abs(bestScore) > MATE - 200, draw: false, only: false,
  };
}

/* ==================== 测试钩子 ==================== */

/** 合法着法树计数(规则的模糊回归哨兵;成五的分支终止计 1) */
export function perft(bd, side, mode, depth) {
  if (depth <= 0) return 1;
  const mvs = legalMoves(bd, side, mode);
  if (depth === 1) return mvs.length;
  let n = 0;
  for (const cell of mvs) {
    make(bd, cell, side);
    if (!madeFive(bd, cell, mode)) n += perft(bd, side ^ 1, mode, depth - 1);
    unmake(bd, cell, side);
  }
  return n;
}

export function nodeCount() { return nodes; }
export function boardToArray(bd) { return Array.from(bd); }
export function arrayToBoard(arr) { const bd = new Int8Array(SIZE); bd.set(arr); syncPosition(bd); return bd; }

/** 测试钩子:暴露全部增量状态,供模糊测试做 make/unmake 一致性断言 */
export function debugState() {
  return {
    hk1: HK1, hk2: HK2, stones, evalScore,
    nei: Array.from(NEI), wb: Array.from(WB), ww: Array.from(WW), cnt: Array.from(CNT),
  };
}
