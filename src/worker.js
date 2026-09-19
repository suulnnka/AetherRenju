/* ============================================================
 * AI Worker:引擎的门面(UI 不 import 引擎源码,一切经消息)
 *   ping                     → { type:'pong', tag }
 *   { type:'levels' }        → { type:'levels', tag, engine, default, levels }
 *                              纯声明难度表,不触发任何引擎加载;
 *                              UI 只读 name/id,参数是引擎的实现细节
 *   { type:'state', id, moves, mode }
 *                            → { type:'state', id, board, stm, forbidden,
 *                                over, winner, cells, reason }
 *                              规则查询的单一入口:重演序列后回报
 *                              棋盘 / 禁手点 / 胜负(含连线)——UI 据此渲染
 *   { type:'think', id, moves, mode, level }
 *                            → 逐层 { id, type:'progress', ... }
 *                            → { id, move, depth, nodes, ms, score, mate }
 *                              level 是**本引擎难度表的下标**(表由 levels 自报),
 *                              UI 不再传 nodes/ms/depth 参数 —— 那些住在引擎层
 *
 * moves 是落点序列(从空盘起,黑白交替)—— 传序列而不是传棋盘:
 * 结构化克隆最省,且编码只有一套(交叉点 0..224),不存在两条解析路径。
 *
 * 搜索是同步的,Worker 收到新消息只会排队;UI 侧用请求序号丢弃过期结果,
 * 需要真正中断时直接 terminate 再造一个(见 webos 应用的 abortEngine)。
 * ============================================================ */
import {
  newBoard, replayMoves, searchBest, checkWin, forbiddenPoints, legalMoves,
  boardToArray, LEVELS, DEFAULT_LEVEL, RENJU, FREE, BLACK, WHITE,
} from './engine.js';

/* ENGINE_TAG 让下游 webos 的体积闸门(check-size.mjs)能在 dist 里认出这个 chunk
 * (字符串不会被压缩改名)。 */
const ENGINE_TAG = 'renju-engine-v1';
self.__engineTag = ENGINE_TAG;

/** 重演序列并产出「UI 渲染所需的全部规则事实」:棋盘、禁手点、胜负与连线。
 *  这是 state 消息的唯一事实源 —— UI 不再自己判禁手 / 判胜,规则只有引擎一份。
 *
 *  replayMoves 的返回约定:-1 非法;-2 = 序列在最后一手成五(有效终局,盘面即
 *  终局盘面);否则 = 轮到谁。成五的胜者与连线在两种路径下都取自 checkWin。 */
function describeState(d) {
  const mode = d.mode === FREE ? FREE : RENJU;
  const bd = newBoard();
  const replayed = replayMoves(bd, d.moves, mode);
  if (replayed === -1) return { error: 'illegal-sequence' };

  const last = d.moves.length ? d.moves[d.moves.length - 1] : -1;
  let over = false, winner = -1, cells = null, reason = null;
  let stm;
  if (replayed === -2) {
    over = true;
    winner = (d.moves.length - 1) % 2;                   // 刚落子的一方(黑先,偶下标 = 黑)
    cells = checkWin(bd, last, mode);
    reason = cells && cells.length > 5 ? 'overline' : 'five';
    stm = winner ^ 1;                                    // 终局后轮次仅供 UI 自洽
  } else {
    stm = replayed;
    /* 胜负由「最后一手」触发:连成五(长连)即胜,连线由 checkWin 给出 */
    const line = last >= 0 ? checkWin(bd, last, mode) : null;
    if (line) {
      over = true;
      winner = 1 - stm;
      cells = line;
      reason = line.length > 5 ? 'overline' : 'five';
    } else if (d.moves.length >= 225) {
      over = true;
      reason = 'full';                                   // 满盘和棋
    } else if (mode === RENJU && stm === BLACK && !legalMoves(bd, BLACK, RENJU).length) {
      over = true;
      winner = WHITE;
      reason = 'no-legal';                               // 黑方被禁手封盘,判负
    }
  }

  return {
    board: boardToArray(bd),
    stm,
    forbidden: (!over && mode === RENJU && stm === BLACK) ? forbiddenPoints(bd) : [],
    over, winner, cells, reason,
  };
}

self.onmessage = (e) => {
  const d = e.data;
  if (!d) return;

  if (d.type === 'ping') { self.postMessage({ type: 'pong', tag: ENGINE_TAG }); return; }

  if (d.type === 'levels') {
    /* 纯声明:难度表(含参数)是引擎的实现细节,UI 只拿 name/id 建下拉 */
    self.postMessage({ type: 'levels', tag: ENGINE_TAG, engine: 'js', default: DEFAULT_LEVEL, levels: LEVELS });
    return;
  }

  if (d.type === 'state') {
    const s = describeState(d);
    self.postMessage(s.error
      ? { type: 'state', id: d.id, error: s.error }
      : { type: 'state', id: d.id, tag: ENGINE_TAG, ...s });
    return;
  }

  const t0 = Date.now();
  const mode = d.mode === FREE ? FREE : RENJU;
  const bd = newBoard();
  const side = replayMoves(bd, d.moves, mode);
  if (side < 0) { self.postMessage({ id: d.id, error: 'illegal-sequence' }); return; }
  const lv = LEVELS[d.level] ?? LEVELS[DEFAULT_LEVEL] ?? LEVELS[0];
  const r = searchBest(bd, side, {
    mode, nodes: lv.nodes, ms: lv.ms, depth: lv.depth, jitter: lv.jitter ?? 0,
  });
  self.postMessage({
    id: d.id, move: r.move, depth: r.depth, nodes: r.nodes,
    ms: Date.now() - t0, score: r.score, mate: !!r.mate,
  });
};
