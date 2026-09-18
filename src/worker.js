/* ============================================================
 * AI Worker:只是一层薄壳
 *   收 { id, moves, mode, nodes, ms, depth, jitter }
 *   逐层回 { id, type:'progress', depth, move, score, nodes, ms }
 *   结束回 { id, move, depth, nodes, ms, score, mate }
 *
 * moves 是落点序列(从空盘起,黑白交替)—— 传序列而不是传棋盘:
 * 结构化克隆最省,且 UI 与 Worker 共用同一份 engine.js,不存在两条规则实现。
 *
 * 搜索是同步的,Worker 收到新消息只会排队;UI 侧用请求序号丢弃过期结果,
 * 需要真正中断时直接 terminate 再造一个(见 webos 应用的 abortEngine)。
 * ============================================================ */
import { newBoard, replayMoves, searchBest, RENJU, FREE } from './engine.js';

/* ENGINE_TAG 让下游 webos 的体积闸门(check-size.mjs)能在 dist 里认出这个 chunk
 * (字符串不会被压缩改名)。 */
const ENGINE_TAG = 'renju-engine-v1';
self.__engineTag = ENGINE_TAG;

self.onmessage = (e) => {
  const d = e.data;
  if (d && d.type === 'ping') { self.postMessage({ type: 'pong', tag: ENGINE_TAG }); return; }
  const t0 = Date.now();
  const mode = d.mode === FREE ? FREE : RENJU;
  const bd = newBoard();
  const side = replayMoves(bd, d.moves, mode);
  if (side < 0) { self.postMessage({ id: d.id, error: 'illegal-sequence' }); return; }
  const r = searchBest(bd, side, {
    mode, nodes: d.nodes, ms: d.ms, depth: d.depth, jitter: d.jitter ?? 0,
  });
  self.postMessage({
    id: d.id, move: r.move, depth: r.depth, nodes: r.nodes,
    ms: Date.now() - t0, score: r.score, mate: !!r.mate,
  });
};
