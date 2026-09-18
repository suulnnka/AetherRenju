# AetherRenju

纯 JavaScript 五子棋 / 连珠引擎:零依赖、无 DOM、浏览器 / Worker / Node 通用。
为 [WebOS](https://github.com/suulnnka/AetherWebOS)(纯前端网页操作系统)的
五子棋应用而写,全部自研。

**在线体验:** 打开 <https://suulnnka.github.io/AetherWebOS/> 启动「五子棋」应用 ——
那里面跑的就是本引擎(窗口信息行实时显示搜索深度 / 评分 / 节点数 / 耗时)。

> v0.1:规则完整(禁手判定与独立暴力判定器逐点对拍),搜索与评估都是
> **第一版、刻意做简单**的。后续路线见 [`docs/ROADMAP.md`](docs/ROADMAP.md)。

## 规则:无禁与有禁

引擎同时支持两套规则,搜索 / 合法着法 / 胜负判定都按 `mode` 走:

| 模式 | 胜负 | 黑方限制 |
|---|---|---|
| `FREE` 无禁(自由五子棋) | 任意一方连成 **≥5 子**即胜(长连也算) | 无 |
| `RENJU` 有禁(连珠) | 黑方**恰好五连**才胜;白方五连或长连都胜 | 长连 / 双四 / 双活三判负(引擎把它们当非法着法,不生成、不搜索) |

禁手判定按 RIF《连珠规则》递归展开(`isForbidden` 顶部注释有完整推导):

- **长连**:六子及以上。
- **四**:补一子能成「恰好五」的形状;同一方向按成五窗的 4 子集合去重
  (活四的两个成五点算同一个四);单线双四(`●●●_x_●●●`)照样识别。
- **三**:存在空点 p,补 p 后出现「过 x 且过 p 的活四」,且补 p 这手
  **本身不成五、不是禁手**(递归)—— 这就是「假活三不算三」的官方口径。
- **五连豁免**:同时成五则一切禁手豁免(黑直接获胜)。

## 棋盘与编码

- 15×15 = 225 点,`idx = 行×15 + 列`;黑先白后,「着法」就是一个点 idx,
  UI 与 Worker 之间只传这一种编码,不搞两套。
- 格值:0 空 / 1 黑 / 2 白;方:0 黑 `BLACK` / 1 白 `WHITE`。
- 模式:`FREE = 0`(无禁)/ `RENJU = 1`(有禁)。

## 引擎

`src/engine.js` 单文件(规则 + 评估 + 搜索),`src/worker.js` 只是 Worker 薄壳。
置换表、killer、history、候选缓冲全是模块级 `TypedArray`,搜索过程**零分配**;
评估是 4 个方向 572 个「五元窗」的计数和,随 make/unmake **增量维护**,叶节点 O(1) 取值。

### 搜索(当前实际用了这些)

| 技术 | 现状 |
|---|---|
| negamax + alpha-beta | 有,但**全窗口** —— 还没做 PVS 零窗口试探 |
| 迭代加深 | 1..depth,每层回调 `onProgress` |
| 置换表 | 2^17 项;双 32 位 Zobrist 校验;存 着法/分数/深度/flag;总是替换 |
| 着法排序 | 候选点增益(落点导致的全盘窗分变化,攻防一体)→ TT 着法 → killer → history |
| 静态搜索 | **VCF 式分级**:己方有成五点直接取胜;对方叫五只许挡(不挡必输);否则才允许自己成四叫杀;链深限 14 ply |
| 禁手 | 懒判定:轮到尝试该点才查,配 (盘面⊕点) 键的小缓存;候选区全被禁死 = 判负 |
| 杀棋 | 每次走子后查成五,立即返回 `MATE - ply - 1`,不再递归 |
| 中断 | 节点预算为主,每 1024 节点查一次墙上时间作兜底 |

**还没做**(见 ROADMAP):PVS、空着裁剪、LMR、威胁空间搜索(VCT)、开局库。

### 评估

五元窗计数:每个窗按「黑子数 b / 白子数 w」计分,混色 0 分、纯黑 `+W[b]`、
纯白 `-W[w]`,权重 `[0, 4, 36, 320, 2800, 1200000]` 为手调初值
(活四 > 冲四、活三 > 眠三的层次靠窗的重叠自然拉)+ tempo 8。
**没有**自对弈拟合 / Texel 调参。

### 难度四档

节点预算为主、墙上时间为兜底(设备无关、可复现);初级另加 root jitter。
下表的「深度」是 `LEVELS[].depth` 上限,**实测**(`bench/bench.mjs nps`,
安静中盘局面,本机 Node 22 / 桌面级 CPU)预算内能稳定走完 2~4 层
(杀棋局面靠静态搜索延伸,远快于此):

| 档位 | 深度上限 | 节点预算 | 实测 NPS |
|---|---|---|---|
| 初级 | 2 | 8k(最优解 ±60 分内随机) | ~9 万 |
| 中级 | 4 | 40k | ~20 万 |
| 高级 | 6 | 200k | ~21 万 |
| 大师 | 8 | 600k | ~29 万 |

## 用法

```js
import {
  BLACK, WHITE, FREE, RENJU, MATE,
  newBoard, sideToMove, make, unmake,
  legalMoves, isForbidden, forbiddenPoints, checkWin, madeFive,
  replayMoves, searchBest, LEVELS,
} from './src/engine.js';

const bd = newBoard();                          // 空盘,黑先
console.log(sideToMove(bd) === BLACK);          // true

make(bd, 7 * 15 + 7, BLACK);                    // 黑下天元
const fbd = forbiddenPoints(bd);                // 有禁模式下黑方禁手点(UI 画 × 用)
const win = checkWin(bd, 7 * 15 + 7, FREE);     // 成五返回整条连线,否则 null

const r = searchBest(bd, WHITE, {
  mode: RENJU,
  ...LEVELS[2],
  onProgress: (i) => console.log(i.depth, i.move, i.score),
});
// r = { move, score, depth, nodes, ms, mate, draw, only }
make(bd, r.move, WHITE);                        // 走子;撤销用 unmake(bd, cell, side)
```

Worker 侧收 `{ id, moves, mode, nodes, ms, depth, jitter }`,回
`{ id, move, depth, nodes, ms, score, mate }`;`moves` 是从空盘起的落点序列,
Worker 自己重演棋盘(结构化克隆最省,且不会有两份规则实现)。

## 测试与基准

```bash
npm test                          # 规则用例 + 禁手用例 + 对拍模糊 + 状态一致性 + 搜索行为 + 随机对局
node bench/bench.mjs nps          # 各档位节点速度
node bench/bench.mjs moves        # 固定深度最佳着法(改搜索/改评估后对拍)
node bench/bench.mjs forbidden    # 禁手判定吞吐(全盘标禁手点场景)
```

**禁手判定的金标准是测试里的独立暴力判定器**:按 RIF 定义逐点枚举、零增量、
零共享缓冲,与引擎的增量实现在随机局面(60 局 × 全部空点)上逐点对拍,
必须完全一致;另有 20 个手工构造的禁手用例(长连 / 双四 / 单线双四 / 双三 /
假活三 / 递归假活三 / 五连豁免 / 四三豁免 / 边角……)。

随机对局模糊测试(双模式各 15 局)验证:胜负与暴力一致、序列可被 Worker 重演、
make/unmake 完整还原全部增量状态、重放与增量重建逐位一致。

## 已知不做(v0.1 的边界)

- **开局库 / 残局库** —— 不内置任何着法表,开局一律进搜索
- **RIF 26 种开局规则**(索索夫等 swap 体系)—— 只做自由开局 + 禁手
- **多线程** —— 单线程到底
- **WASM** —— 暂未启动,等 JS 侧优化到头再评估

> 体积预算 35 KB gzip(与 AetherChess / AetherXiangqi 同档),由 WebOS 侧
> `tools/check-size.mjs` 在 `npm run build` 时拦;当前约 11 KB。

## License

MIT
