/* AI Worker:wasm 引擎门面。消息契约与旧 JS 版逐字兼容
 * (ping / levels / state / think,详见 README);moves 是从空盘
 * 起的落点序列(黑白交替,0..224)。state 回包新增可选 `opening`
 * (26 开局名,仅显示);think 回包新增 `book`(前三手的开局策略
 * 着法,trivial bestmove,不走搜索、无统计)。引擎本体在
 * ../wasm/aether_renju.wasm,静态区不增长,视图实例化后稳定。 */
const ENGINE_TAG = 'renju-engine-v2';
self.__engineTag = ENGINE_TAG;

const FREE = 0, RENJU = 1, BLACK = 0;
const DEFAULT_LEVEL = 2;
const REASON = { 1: 'five', 2: 'overline', 3: 'full', 4: 'no-legal' };
const NAMES = [
  { id: 'easy', name: '初级', desc: '2 层搜索 + 随机化' },
  { id: 'normal', name: '中级', desc: '4 层迭代加深' },
  { id: 'hard', name: '高级', desc: '6 层迭代加深' },
  { id: 'master', name: '大师', desc: '8 层迭代加深' },
];

let EX = null, IN_ADDR = 0, IN = null, STATE = null, SEARCH = null, LEVELS = null;

const ready = (async () => {
  const { instance } = await WebAssembly.instantiateStreaming(
    fetch(new URL('../wasm/aether_renju.wasm', import.meta.url)),
    { env: { now: () => performance.now() } },
  );
  EX = instance.exports;
  if (EX.ar_version() !== 2) throw new Error('engine ABI mismatch');
  const mem = EX.memory.buffer;
  IN_ADDR = EX.ar_in_ptr();
  IN = new Uint8Array(mem, IN_ADDR, 257);
  STATE = new Int32Array(mem, EX.ar_state_ptr(), EX.ar_state_len());
  SEARCH = new Int32Array(mem, EX.ar_search_ptr(), EX.ar_search_len());
  LEVELS = new Int32Array(mem, EX.ar_levels_ptr(), EX.ar_levels_len());
})();

function feed(moves) {
  IN.set(moves);
  return moves.length;
}

/** 重演序列并产出 UI 渲染所需的全部规则事实(引擎单源) */
function describeState(moves, mode) {
  const n = feed(moves);
  if (EX.ar_state(IN_ADDR, n, mode) !== 0) return { error: 'illegal-sequence' };
  const over = STATE[1], winner = STATE[2], lineLen = STATE[4];
  /* 开局库(仅显示,协议参考 AetherOthello):前三手命中 26 开局时
   * 引擎给出名字(线性内存静态串,按 ptr/len 零拷贝读 UTF-8);三手
   * 之后不失效(开局属于整局)。旧 wasm 无此导出时静默缺省。 */
  let opening = null;
  if (n >= 3 && typeof EX.ar_opening === 'function' && EX.ar_opening(IN_ADDR, n) > 0) {
    const p = EX.ar_opening_name_ptr(), l = EX.ar_opening_name_len();
    if (p > 0 && l > 0) {
      opening = new TextDecoder().decode(new Uint8Array(EX.memory.buffer, p, l));
    }
  }
  return {
    board: Array.from(STATE.slice(15, 15 + 225)),
    stm: STATE[0],
    forbidden: STATE[240] > 0 ? Array.from(STATE.slice(241, 241 + STATE[240])) : [],
    over: !!over,
    winner,
    cells: lineLen > 0 ? Array.from(STATE.slice(5, 5 + lineLen)) : null,
    reason: REASON[STATE[3]] || null,
    ...(opening ? { opening } : {}),
  };
}

self.onmessage = async (e) => {
  const d = e.data;
  if (!d) return;
  try {
    await ready;
  } catch (err) {
    self.postMessage({ id: d.id, error: 'engine-init: ' + err.message });
    return;
  }

  if (d.type === 'ping') {
    self.postMessage({ type: 'pong', tag: ENGINE_TAG });
    return;
  }

  if (d.type === 'levels') {
    /* 参数(depth/nodes/ms/jitter)是引擎自报表;UI 只拿 name/id 建下拉 */
    const levels = [];
    for (let i = 0; i < LEVELS.length; i += 4) {
      levels.push({
        ...(NAMES[i / 4] || { id: 'lv' + i / 4, name: '档位' + (i / 4 + 1) }),
        depth: LEVELS[i], nodes: LEVELS[i + 1], ms: LEVELS[i + 2], jitter: LEVELS[i + 3],
      });
    }
    self.postMessage({ type: 'levels', tag: ENGINE_TAG, engine: 'wasm', default: DEFAULT_LEVEL, levels });
    return;
  }

  if (d.type === 'state') {
    const s = describeState(d.moves, d.mode === FREE ? FREE : RENJU);
    self.postMessage(s.error
      ? { type: 'state', id: d.id, error: s.error }
      : { type: 'state', id: d.id, tag: ENGINE_TAG, ...s });
    return;
  }

  /* think:level 是引擎难度表下标 */
  const t0 = Date.now();
  const mode = d.mode === FREE ? FREE : RENJU;
  const n = feed(d.moves);
  const lv = [];
  {
    const i = Math.max(0, Math.min(LEVELS.length / 4 - 1, d.level | 0)) * 4;
    lv.push(LEVELS[i], LEVELS[i + 1], LEVELS[i + 2], LEVELS[i + 3]);
  }
  const [depth, nodeBudget, msBudget, jitter] = lv;
  if (EX.ar_search(IN_ADDR, n, mode, depth, nodeBudget, msBudget) !== 0) {
    self.postMessage({ id: d.id, error: 'illegal-sequence' });
    return;
  }
  let move = SEARCH[0], score = SEARCH[1];
  const flags = SEARCH[4], rootLen = SEARCH[5];
  /* 低难度:在最优解附近随机挑一个(jitter 池来自引擎根分数表)。
   * 开局策略着法(flags bit3)rootLen = 0,自然跳过 */
  if (!(flags & 8) && jitter > 0 && score > -29800 && rootLen > 1) {
    const pool = [];
    for (let i = 0; i < rootLen && SEARCH[6 + 2 * i + 1] >= score - jitter; i++) {
      pool.push(SEARCH[6 + 2 * i]);
    }
    if (pool.length > 1) move = pool[(Math.random() * pool.length) | 0];
  }
  self.postMessage({
    id: d.id, move, depth: SEARCH[2], nodes: SEARCH[3],
    ms: Date.now() - t0, score, mate: !!(flags & 1), book: !!(flags & 8),
  });
};
