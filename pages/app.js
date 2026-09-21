/* ============================================================
 * 五子棋在线对弈页 —— UI 移植自 AetherWebOS 的五子棋应用
 * (js/apps/gomoku/index.js),布局与交互保持一致:
 * 顶栏「新对局 / 规则 / 难度 / 人机 / 换边 / 悔棋」+ 中央棋盘 +
 * 底栏左侧行棋状态、右侧等宽字体引擎搜索信息。
 *
 * 引擎即本仓库的主角:src/worker.js(纯 JS,alpha-beta + 置换表)。
 * UI 一行引擎代码都不 import:难度表、棋盘事实(禁手点 / 胜负连线 /
 * 禁手封盘)全部经 Worker 消息问引擎 —— 规则只有引擎一份。
 *
 * UI 持有的唯一对局状态是**落点序列**(交叉点 0..224):落子 / 悔棋 /
 * 新对局都只是改序列再向 Worker 要一次 state 回包,拿回棋盘与禁手重画。
 * ============================================================ */

/* ==================== 微型工具(替代 webos 的 core)==================== */
const $ = (sel) => document.querySelector(sel);

/** 建 DOM:el('button', {class, onClick, dataset}, ...children) */
function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null) continue;
    if (k === 'class') node.className = v;
    else if (k === 'dataset') Object.assign(node.dataset, v);
    else if (k.startsWith('on') && typeof v === 'function') node.addEventListener(k.slice(2).toLowerCase(), v);
    else if (k === 'style' && typeof v === 'object') Object.assign(node.style, v);
    else node.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

/** 线性图标(路径数据取自 webos 的 core/icons.js) */
const ICON_PATHS = {
  refresh: '<path d="M21 12a9 9 0 1 1-2.64-6.36L21 8"/><path d="M21 3v5h-5"/>',
  reply: '<polyline points="9 17 4 12 9 7"/><path d="M20 18v-2a4 4 0 0 0-4-4H4"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
  moon: '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z"/>',
};
const icon = (name) => {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('fill', 'none');
  s.setAttribute('stroke', 'currentColor');
  s.setAttribute('stroke-width', '2');
  s.setAttribute('stroke-linecap', 'round');
  s.setAttribute('stroke-linejoin', 'round');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = ICON_PATHS[name] || '';
  return s;
};

/** webos dialogs.info 的页内替身 */
const dlg = $('#dlg');
function showDialog({ title, message }) {
  $('#dlgTitle').textContent = title;
  $('#dlgMsg').textContent = message;
  if (!dlg.open) dlg.showModal();
}
$('#dlgOk').addEventListener('click', () => dlg.close());
dlg.addEventListener('click', (e) => { if (e.target === dlg) dlg.close(); });

/** webos bus.notify 的页内替身:右下角吐司 */
function toast(text) {
  const t = el('div', { class: 'toast' }, text);
  t.addEventListener('click', () => t.remove());
  $('#toasts').append(t);
  setTimeout(() => { t.classList.add('out'); setTimeout(() => t.remove(), 220); }, 3200);
}

/** 主题:webos 的浅 / 深双主题,记在 localStorage */
const themeBtn = $('#themeBtn');
function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  themeBtn.replaceChildren(icon(theme === 'dark' ? 'sun' : 'moon'));
  try { localStorage.setItem('aether-pages-theme', theme); } catch {}
}
applyTheme((() => {
  try { return localStorage.getItem('aether-pages-theme') || 'dark'; } catch { return 'dark'; }
})());
themeBtn.addEventListener('click', () =>
  applyTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'));

const setTitle = (t) => { $('#winTitle').textContent = t; };

/* ==================== 对局(逻辑同 webos 五子棋应用)==================== */

/* 协议常量(worker 契约的一部分,不是引擎导出):黑先白后,黑白交替 */
const BLACK = 0, WHITE = 1;
const FREE = 0, RENJU = 1;                 // 规则模式:无禁 / 有禁(连珠)

/* 格距(px)。与 style.css 里的 --cs / --pad 必须一致 */
const CS = 34, PAD = 22;
const T = PAD * 2 + CS * 14;
const X = (c) => PAD + c * CS;
const Y = (r) => PAD + r * CS;
const COLS = 'ABCDEFGHIJKLMNO';
const sideName = (s) => (s === BLACK ? '黑方' : '白方');
const coordText = (i) => COLS[i % 15] + (15 - ((i / 15) | 0));

/** 引擎评分(窗分,行棋方视角)→ 给人看的字符串。±20000 以上是杀棋分,只标 M */
const fmtScore = (s) => {
  if (Math.abs(s) >= 20000) return s > 0 ? '+M' : '-M';
  return (s >= 0 ? '+' : '') + s;
};

/** 棋盘线(SVG):15×15 线 + 天元与四星 + 边缘坐标(下 A~O、左 15~1) */
function boardSvg() {
  const d = [];
  for (let i = 0; i < 15; i++) {
    d.push(`M${X(0)} ${Y(i)}H${X(14)}`);
    d.push(`M${X(i)} ${Y(0)}V${X(14)}`);
  }
  const stars = [[3, 3], [3, 11], [11, 3], [11, 11], [7, 7]]
    .map(([r, c]) => `<circle class="gk-star" cx="${X(c)}" cy="${Y(r)}" r="3"/>`)
    .join('');
  const coords = [];
  for (let c = 0; c < 15; c++) {
    coords.push(`<text class="gk-coord" x="${X(c)}" y="${T - PAD / 2}">${COLS[c]}</text>`);
    coords.push(`<text class="gk-coord" x="${PAD / 2}" y="${Y(c)}">${15 - c}</text>`);
  }
  return `<svg class="gk-lines" viewBox="0 0 ${T} ${T}" aria-hidden="true">`
    + `<path class="gk-line" d="${d.join(' ')}"/>${stars}${coords.join('')}</svg>`;
}

const appEl = $('#app');

let board = new Array(225).fill(0);   // 由 Worker 的 state 回包驱动(0 空 / 1 黑 / 2 白)
let bans = new Set();                 // 当前局面的黑方禁手点(state 回包)
let turn = BLACK;                     // 黑先
let humanSide = BLACK;                // 玩家执子方(换边可改);棋盘对称,不翻转
let mode = RENJU;                     // FREE 无禁 / RENJU 有禁
let hist = [];                        // 落点序列 —— UI 持有的唯一对局状态
let lastMove = null;
let winLine = null;                   // 胜利连线(高亮)
let gameOver = false;
let vsAI = true;
let searching = false;
let levels = [];                      // 难度表由**引擎自报**({type:'levels'})
let levelIdx = 0;

const aiSide = () => humanSide ^ 1;
const lvName = () => levels[levelIdx]?.name ?? '—';

/* 终局弹窗缓冲(600ms):终局画面先落地,给玩家一点反应时间再弹结算。
 * 缓冲期里的新对局 / 悔棋 / 换边 / 人机切换都调 cancelEndDlg 取消 ——
 * 不然这些操作之后还会蹦出上一局的结算框。 */
const END_DLG_MS = 600;
let endDlgTimer = 0;
const cancelEndDlg = () => { clearTimeout(endDlgTimer); endDlgTimer = 0; };
const popEndDlg = (show) => {
  cancelEndDlg();
  endDlgTimer = setTimeout(() => { endDlgTimer = 0; show(); }, END_DLG_MS);
};

const statusL = el('span', {}, '黑方行棋');
const infoL = el('span', {
  class: 'mono', style: { fontSize: '11px' },
  title: '引擎搜索信息(评分是 AI 视角,单位窗分;+M / -M 表示算到杀棋)',
}, '');
const layerEl = el('div', { class: 'gk-layer' });
const boardEl = el('div', { class: 'gk-board' }, layerEl);
const fitWrap = el('div', { class: 'fit-wrap' }, boardEl);

/** 棋盘按可用空间等比缩放(棋盘内部是固定像素布局) */
function fitBoard() {
  const body = appEl.querySelector('.app-body');
  if (!body) return;
  const w = body.clientWidth - 24, h = body.clientHeight - 24;
  const bw = boardEl.offsetWidth, bh = boardEl.offsetHeight;
  if (!bw || !bh) return;
  fitWrap.style.transform = `scale(${Math.min(1, w / bw, h / bh)})`;
}

/* ---------- 渲染(全部基于最近一次 state 回包的缓存) ---------- */
function render() {
  layerEl.innerHTML = boardSvg();
  /* 虚影与可点光标只在「轮到玩家」时出现;禁手点标 × 且永远不可落 */
  const humanTurn = !gameOver && (!vsAI || turn === humanSide);
  boardEl.classList.toggle('turn-b', humanTurn && turn === BLACK);
  boardEl.classList.toggle('turn-w', humanTurn && turn === WHITE);
  for (let i = 0; i < 225; i++) {
    const btn = el('button', {
      class: 'gk-pt'
        + (!gameOver && !board[i] && !(bans.has(i)) ? ' can' : '')
        + (bans.has(i) ? ' ban' : '')
        + (winLine && winLine.includes(i) ? ' gk-win' : ''),
      style: { left: X(i % 15) + 'px', top: Y((i / 15) | 0) + 'px' },
      dataset: { i: String(i) },
      onClick: () => onPoint(i),
    });
    if (board[i]) {
      const st = el('div', {
        class: `gk-stone ${board[i] === 1 ? 'black' : 'white'}${i === lastMove ? ' last' : ''}`,
      });
      if (i === lastMove) st.classList.add('drop');
      btn.append(st);
    } else if (humanTurn) {
      btn.append(el('div', { class: 'gk-ghost' }));
    }
    if (bans.has(i) && !board[i]) btn.append(el('div', { class: 'gk-ban' }));
    layerEl.append(btn);
  }
}

function updateStatus() {
  if (gameOver) return;
  const banNote = mode === RENJU && turn === BLACK && bans.size
    ? ' · ×为禁手' : '';
  statusL.textContent = `${sideName(turn)}行棋 · 第 ${hist.length + 1} 手${banNote}`;
  const last = hist.length ? ` · 上一手 ${coordText(hist[hist.length - 1])}` : '';
  setTitle(`五子棋 — ${sideName(turn)}行棋${last}`);
}

/* ---------- 落子:合法性以缓存 state 为准,走子 = 改序列 + 再问一次引擎 ---------- */
function onPoint(i) {
  if (gameOver || board[i] || statePending) return;
  if (vsAI && turn !== humanSide) return;     // AI 回合/思考中不响应点击
  if (mode === RENJU && turn === BLACK && bans.has(i)) {
    toast('五子棋:' + `落点 ${coordText(i)} 是黑方禁手(三三 / 四四 / 长连)`);
    return;
  }
  doMove(i);
}

function doMove(cell) {
  hist.push(cell);
  lastMove = cell;
  turn ^= 1;
  fetchState();
}

/** state 回包落地:重画 + 按回包事实终局 / 调度 AI */
function applyState(d) {
  board = d.board;
  bans = new Set(d.forbidden);
  turn = d.stm;
  if (d.over) {
    if (d.reason === 'full') endDraw();
    else if (d.reason === 'no-legal') endGame(d.winner, null, 'no-legal');
    else endGame(d.winner, d.cells);
    return;
  }
  render();
  if (!gameOver && vsAI && turn === aiSide()) setTimeout(thinkAI, 260);
  else updateStatus();
}

function endGame(winner, line, whyOverride) {
  gameOver = true;
  winLine = line;
  abortEngine();
  render();
  const why = whyOverride === 'no-legal' ? '禁手封盘' : (line && line.length > 5 ? '长连' : '五连');
  const who = sideName(winner) + (vsAI && winner === aiSide() ? '(AI)' : '');
  const line2 = `${why} — ${sideName(winner)}胜`;   // 状态行不标 (AI),只说哪方胜
  const detail = whyOverride === 'no-legal'
    ? `${who}获胜!(对方无合法落点)`
    : `${who} ${why}获胜!(${line.length} 子连线)`;
  /* 结算弹窗缓一拍:让玩家看清胜利连线再弹;缓冲期里的操作会取消它 */
  popEndDlg(() => showDialog({ title: '终局', message: detail }));
  statusL.textContent = line2;
  setTitle('五子棋 — 终局');
  toast('五子棋:' + line2);
}

function endDraw() {
  gameOver = true;
  abortEngine();
  statusL.textContent = '满盘 — 和棋';
  setTitle('五子棋 — 终局');
  popEndDlg(() => showDialog({ title: '终局', message: '棋盘已满,和棋' }));
  toast('五子棋:满盘和棋');
}

/* ---------- Worker:难度表 / 局面事实 / 搜索都经它 ---------- */
let worker = null, reqSeq = 0, stateSeq = 0, statePending = null;

function killWorker() {
  if (worker) { worker.terminate(); worker = null; }
  searching = false;
  if (statePending) { const p = statePending; statePending = null; p(null); }
  reqSeq++;    // 作废已进主线程队列的旧结果
}

/** 作废在途请求(局面已变 / 页面关闭),免得过期着法落到新对局上 */
function abortEngine() { killWorker(); infoL.textContent = ''; }

function ensureWorker() {
  if (worker) return worker;
  try {
    /* pages/app.js 的上一级就是仓库根:本地仓库起服与 GitHub Pages 的
     * _site 是同一布局,相对路径在两边走的是同一套 */
    worker = new Worker(new URL('../src/worker.js', import.meta.url), { type: 'module' });
  } catch (err) {
    console.error('[gomoku-pages] 无法创建 AI Worker:', err);
    worker = null; searching = false;
    statusL.textContent = 'AI 不可用(Worker 创建失败)';
    return null;
  }
  worker.onmessage = onEngineMsg;
  worker.onerror = (ev) => {
    console.warn('[gomoku-pages] AI Worker 异常:', ev.message || ev);
    killWorker();
    statusL.textContent = 'AI 出错,已跳过本步';
  };
  return worker;
}

function onEngineMsg(e) {
  const d = e.data;
  if (!d) return;
  if (d.type === 'levels') { applyLevels(d); return; }
  if (d.type === 'state') {
    if (!statePending || d.id !== stateSeq) return;   // 过期局面直接丢
    const p = statePending; statePending = null;
    p(d.error ? null : d);
    return;
  }
  /* ---- 以下是搜索回包(progress / 最终结果)---- */
  if (d.id !== reqSeq) return;
  if (d.type === 'progress') { showInfo(d); return; }
  searching = false;
  if (d.error) { statusL.textContent = '引擎异常:' + d.error; return; }
  hist.push(d.move);
  lastMove = d.move;
  turn ^= 1;
  showInfo(d);
  fetchState();
}

/** 向 Worker 要当前局面的规则事实(state 契约) */
function fetchState() {
  if (!ensureWorker()) return;
  const id = ++stateSeq;
  statePending = (d) => {
    if (!d) return;                           // 被作废(terminate / 新对局)
    applyState(d);
  };
  worker.postMessage({ type: 'state', id, moves: hist.slice(), mode });
}

/** 开局问一次引擎的难度表,拿到才填下拉 */
function applyLevels(d) {
  const table = Array.isArray(d.levels)
    ? d.levels.filter((lv) => lv && typeof lv.name === 'string' && lv.name) : [];
  if (!table.length) {
    levelSel.title = 'AI 难度不可用(引擎未上报)';
    return;
  }
  levels = table;
  const def = Number.isInteger(d.default) && d.default >= 0 && d.default < table.length ? d.default : 0;
  levelIdx = def;
  levelSel.append(...table.map((lv, i) => el('option', { value: String(i) }, lv.name)));
  levelSel.value = String(def);
  levelSel.disabled = false;
  levelSel.title = 'AI 难度:' + table.map((lv) => lv.name).join(' / ');
}

function thinkAI() {
  if (gameOver || searching) return;
  searching = true;
  render();
  statusL.textContent = `${sideName(aiSide())}思考中…`;
  setTitle(`五子棋 — AI 思考中(${lvName()})`);
  infoL.textContent = '';
  if (typeof Worker === 'undefined') {
    searching = false;
    statusL.textContent = '当前环境不支持 Web Worker,AI 不可用';
    return;
  }
  if (!ensureWorker()) return;
  worker.postMessage({ id: ++reqSeq, moves: hist.slice(), mode, level: levelIdx });
}

/** 底栏右侧的引擎信息行(等宽字体) */
function showInfo(d) {
  infoL.textContent = `${lvName()} · 深度 ${d.depth} · `
    + `${Math.round(d.nodes / 1000)}k 节点 · ${d.ms}ms · ${fmtScore(d.score)}`;
}

/* ---------- 工具栏动作 ---------- */
function resetGame() {
  abortEngine();
  cancelEndDlg();
  turn = BLACK; hist = []; lastMove = null; winLine = null;
  gameOver = false;
  board = new Array(225).fill(0); bans = new Set();
  render();
  fetchState();                                // 初始局面事实照问引擎
  if (vsAI && turn === aiSide()) thinkAI();    // 玩家执白时 AI 执黑先行
  else updateStatus();
}

/** 悔棋:撤到「轮到玩家重新决策」为止。人机撤两手,人人撤一手 */
function doUndo() {
  if (!hist.length) return;
  abortEngine();
  cancelEndDlg();
  let n = 1;
  if (vsAI && turn === humanSide && hist.length >= 2) n = 2;
  while (n-- > 0 && hist.length) hist.pop();
  turn = hist.length % 2 === 0 ? BLACK : WHITE;
  gameOver = false; winLine = null;
  lastMove = hist.length ? hist[hist.length - 1] : null;
  fetchState();
  if (vsAI && turn === aiSide()) thinkAI();
  else { render(); updateStatus(); }
}

/** 换边:与 AI 互换执子方。棋盘对称不翻转 */
function switchSide() {
  abortEngine();
  cancelEndDlg();
  humanSide ^= 1;
  render();
  if (!gameOver && vsAI && turn === aiSide()) thinkAI();
  else if (!gameOver) updateStatus();
}

/* ---------- 界面 ---------- */
const newBtn = el('button', { class: 'btn primary', onClick: resetGame }, icon('refresh'), '新对局');
/* 规则档:无禁(自由)/ 有禁(连珠)。切规则即开新对局 —— 禁手影响合法性,
 * 中途切换容易让「刚才还能走的点」变得走不得,重开最干净。 */
const modeSel = el('select', {
  class: 'select gk-mode',
  title: '规则:无禁手 = 长连也算胜;有禁手 = 黑方三三/四四/长连判负',
  'aria-label': '规则',
  onChange: (e) => {
    const next = Number(e.currentTarget.value);
    if (next === mode) return;
    mode = next;
    if (hist.length) toast('五子棋:规则已切换,开新对局');
    resetGame();
  },
},
  el('option', { value: String(RENJU) }, '有禁手'),
  el('option', { value: String(FREE) }, '无禁手'));
modeSel.value = String(mode);                 // 默认「有禁手」

const levelSel = el('select', {
  class: 'select gk-level',
  title: 'AI 难度(等引擎上报)',
  'aria-label': 'AI 难度',
  disabled: true,
  onChange: (e) => {
    levelIdx = Number(e.currentTarget.value) || 0;
    if (searching) { abortEngine(); thinkAI(); }
  },
});
const aiBtn = el('button', {
  class: 'btn', title: '切换人机 / 双人对弈',
  onClick: (e) => {
    cancelEndDlg();
    vsAI = !vsAI;
    e.currentTarget.replaceChildren(vsAI ? '人机' : '双人');
    sideBtn.disabled = !vsAI;                                 // 换边只对人机模式有意义
    if (!vsAI) { abortEngine(); render(); updateStatus(); }   // 关掉 AI 要把在途搜索停掉
    else if (!gameOver && turn === aiSide()) thinkAI();       // 轮到 AI 就立刻接手
    else updateStatus();
  },
}, '人机');
const sideBtn = el('button', {
  class: 'btn', title: '换边:与 AI 互换执子方(棋盘对称,不翻转)',
  onClick: switchSide,
}, '换边');
const undoBtn = el('button', {
  class: 'btn', title: '悔棋:人机模式连 AI 的应手一起撤,人人模式撤一手',
  onClick: doUndo,
}, icon('reply'), '悔棋');

appEl.append(el('div', { class: 'app' },
  el('div', { class: 'app-toolbar' },
    newBtn,
    el('label', { class: 'gk-mode-wrap', title: '规则' },
      el('span', { class: 'dim', style: { fontSize: '12px' } }, '规则'), modeSel),
    el('label', { class: 'gk-level-wrap', title: 'AI 难度' },
      el('span', { class: 'dim', style: { fontSize: '12px' } }, '难度'), levelSel),
    aiBtn, sideBtn, undoBtn),
  el('div', { class: 'app-body' }, fitWrap),
  el('div', { class: 'app-status' }, statusL,
    el('span', { class: 'grow' }),
    infoL)));

render();
updateStatus();
(function fetchLevels() {
  if (!ensureWorker()) return;
  worker.postMessage({ type: 'levels' });      // 回包经 onEngineMsg → applyLevels
})();
fetchState();                                  // 初始局面的禁手点等事实也要问引擎
new ResizeObserver(fitBoard).observe(appEl.querySelector('.app-body'));
fitBoard();

/* 页面冒烟探针钩子(验证脚本用) */
window.__pagesStats = () => ({
  plies: hist.length, turn, human: humanSide, gameOver, vsAI,
  mode: mode === RENJU ? 'renju' : 'free', level: lvName(),
});
window.__pagesHumanMove = () => {
  if (gameOver || (vsAI && turn !== humanSide)) return false;
  /* 天元附近先手(112 = 中心点),被占就退而求其次:第一个空点 */
  const i = !board[112] && !(mode === RENJU && turn === BLACK && bans.has(112)) ? 112 : board.findIndex((v, j) => !v);
  if (i < 0) return false;
  doMove(i);
  return true;
};
