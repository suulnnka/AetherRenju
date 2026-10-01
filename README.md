# AetherRenju

连珠 / 五子棋引擎:**Rust 编写,编译为 WebAssembly**,浏览器 Worker 里跑。
为 [WebOS](https://github.com/suulnnka/AetherWebOS) 的五子棋应用而写,规则、
搜索与评估全部自研。

**在线体验:** 打开 <https://suulnnka.github.io/AetherRenju/> 开箱即玩;
WebOS 里的「五子棋」应用跑的也是本引擎。

## v0.2:Rust + WebAssembly 重写

v0.1 是纯 JS 单文件引擎;v0.2 起引擎本体用 Rust 重写并编译为 wasm,
旧 `src/engine.js` 已删除。**Worker 消息契约逐字兼容** —— `pages/app.js`
与 WebOS 应用侧零改动,只是引擎的门面从 JS 换成了 wasm:

| | v0.1(JS) | v0.2(Rust → wasm) |
|---|---|---|
| 引擎产物 | `src/engine.js` | `wasm/aether_renju.wasm` |
| gzip 体积 | ~11 KB(预算 35 KB) | **~19 KB(预算 70 KB)** |
| 搜索 | negamax α-β + 迭代加深 + TT + killer/history | + PVS、期望窗口、双槽 TT、LMR/LMP/IIR、连续历史、**根 VCT 证明搜索** |
| 实测 NPS(安静中盘) | 20~29 万 | **32~52 万** |
| 规则 / 禁手 | RIF 递归禁手 + 暴力对拍 | 完整移植,对拍金标准不变 |

取舍:学习型大网络评估放不进 70 KB 预算,评估走「线形分类 + 小权重表」
路线(16 类线形 × 4 向组合,权重表 gzip 后内嵌,详见 `src/eval.rs`);
搜索侧(PVS / 期望窗口 / 双槽 TT / 威胁门控 LMR / 威胁分级排序 /
连续历史 / 根 VCT)按节点预算为主的可复现性约束收敛。

## 规则:无禁与有禁

| 模式 | 胜负 | 黑方限制 |
|---|---|---|
| `FREE` 无禁(自由五子棋) | 任意一方连成 **≥5 子**即胜(长连也算) | 无 |
| `RENJU` 有禁(连珠) | 黑方**恰好五连**才胜;白方五连或长连都胜 | 长连 / 双四 / 双活三判负(引擎把它们当非法着法,不生成、不搜索) |

禁手判定按 RIF《连珠规则》递归展开(`src/lib.rs` 的 `is_forbidden` 注释有
完整推导):长连、四(成五窗子集去重,单线双四照样识别)、三(补点递归判定,
假活三不算三)、五连豁免一切禁手。

## 棋盘与编码

- 15×15 = 225 点,`idx = 行×15 + 列`;黑先白后。
- 格值 0 空 / 1 黑 / 2 白;模式 `FREE = 0` / `RENJU = 1`。
- UI 与 Worker 之间只传落点序列一种编码,不搞两套。

## 架构

```
Rust(crate aether-renju,wasm32 上 no_std、零动态分配)
├── src/lib.rs      规则核心:增量棋盘 / RIF 递归禁手
├── src/eval.rs     静态评估:16 类线形分类 + 格码权重表 + 威胁掩码
├── src/search.rs   PVS 迭代加深 + 期望窗口 + 双槽置换表 + LMR/LMP/IIR
│                   + 威胁分级排序 + killer/history/连续历史 + 静态搜索
├── src/vct.rs      根 VCT 证明搜索(AND-OR,负结论专用 TT,反击四修正)
├── src/opening.rs  26 开局名表 + 开局策略(前三手 trivial
│                   bestmove 直接出着不走搜索;搜索阶段不读开局库)
├── src/api.rs      原始 C ABI 导出(不用 wasm-bindgen,体积最小)
└── src/worker.js   Worker 门面:加载 wasm,消息契约与旧版逐字兼容
```

### 搜索清单(当前实际用了这些)

| 技术 | 说明 |
|---|---|
| PVS + 迭代加深 | 首着全窗口,其余零窗口 + 条件重搜 |
| 期望窗口 | 深度 ≥3 用上轮分 ±240 窗,失败倍增扩窗 |
| 置换表 | 2^16 桶双槽(depth-preferred + always-replace 下压),每步清零 |
| LMR(威胁门控) | 安静着才减排;威胁着 / killer 永不减排 |
| LMP / IIR | 浅层安静着计数剪枝 / TT 未命中深节点减层 |
| 着法排序 | 威胁分级 → 增益 → killer → history → 连续历史 |
| 静态搜索 | 己方成五点 → 对方叫五必堵 → 己方成四叫杀 |
| 根 VCT | 1/8 预算 AND-OR 证明(深度 12),防守集含反击四,证明即报杀 |
| 预算 | 节点预算为主(可复现),墙上时间兜底 |

### 评估

线形分类(`src/eval.rs`):每格每方向判 16 类线形(死形/长连/眠活一二三
四/成五,有禁黑方含禁手语义),四向排序取格码查权重表,叠加双方威胁
掩码修正,相邻两手平滑;分类器窗口试放 + 记忆化,增量维护只重算落子
±5 线内空格,叶节点 O(1)。权重表(CC0 数据,Texel 式调参)内嵌
`src/eval_tables.bin`。

### 难度四档(参数由 wasm 引擎自报,Worker 只补 UI 文案)

初级(深度 2 / 8k 节点,±60 分随机)、中级(4 / 40k)、高级(6 / 200k,
默认档)、大师(8 / 600k,预算内可到 7 层 + VCT 延伸)。

## 构建与体积

```bash
cargo build --release --target wasm32-unknown-unknown   # 或 ./scripts/build.sh
npm run build        # 构建 + 拷贝到 wasm/ + 体积闸门
npm run size         # 单独跑体积闸门
```

体积闸门(`scripts/check-size.mjs`):**引擎 wasm gzip 后 ≤ 70 KB**,
当前实测约 19 KB。手段:wasm32 上 `no_std` + 全静态缓冲(线性内存不
增长)+ 原始 C ABI(不用 wasm-bindgen)+ `opt-level=z` / LTO / `panic=abort`;
置换表等大数组全零初始化,不占文件体积。

## 测试与基准

```bash
npm test                 # cargo test(Rust 侧全量)+ node 冒烟(wasm ABI + Worker 契约)
npm run bench            # 各档位节点速度
npm run bench:moves      # 固定深度最佳着法(改搜索/评估后对拍)
npm run bench:forbidden  # 禁手标记吞吐
```

**禁手判定的金标准**是 Rust 测试里的独立暴力判定器(`tests/engine.rs`):
按 RIF 定义逐点枚举、零增量、零共享缓冲,与引擎的增量实现在随机局面上
逐点对拍,必须完全一致;另有 20 个手工构造的禁手用例(长连 / 双四 /
单线双四 / 双三 / 假活三 / 递归假活三 / 五连豁免 / 边角……)。
其余:胜负对拍、make/unmake 状态一致性、重演一致性、perft、随机对局
(双模式)、VCT 强制胜、api 层 state/search 输出、26 开局表与形状匹配
(独立几何实现穷举 184 个合法位形对拍,`tests/opening.rs`)。

## Worker 契约(v0.1 起不变;`opening` 为后加的可选字段)

- `ping` → `{type:'pong', tag}`
- `{type:'levels'}` → `{type:'levels', tag, engine, default, levels}`
- `{type:'state', id, moves, mode}` → `{type:'state', id, board, stm, forbidden, over, winner, cells, reason, opening?}`(非法序列 → `error:'illegal-sequence'`)
- `{id, moves, mode, level}` → `{id, move, depth, nodes, ms, score, mate, book}`

`moves` 是从空盘起的落点序列(黑白交替);搜索是同步的,UI 用请求序号
丢弃过期结果,需要真正中断时 terminate 再造。

`book = true` 的着法来自**开局策略**(trivial bestmove:前三手
且位形在 26 开局域内 —— 空盘天元 / 一子紧邻 / 两子在 5×5 区内 ——
直接给点不走搜索,`depth`/`nodes` 为 0,选点在合法开局形状内随机,
随机源是 `env.now`);书外位形自动回退搜索,`ar_book_enable(0)` 可关
(基准用,让前三手也走搜索)。

`state` 回包的 `opening`(可选):前三手命中连珠 26 开局时的开局名
(如 `'花月'`),字段三手起整局携带;UI 在**盘面 ≤5 手**(开局阶段)
时把它与引擎搜索信息同栏显示 ——「开局库 · 花月 · 高级 · 深度 6 ·
84k 节点 · 300ms · +12」,第 6 手起隐藏;策略着法无统计时只显示
「开局库 · 花月」。引擎导出 `ar_opening` + `ar_opening_name_ptr/len`,
名字零拷贝,协议形态参考 AetherOthello 的开局书门面;数据来源见
`book/README.md`。

## 已知不做(v0.2 的边界)

- **评估型开局书 / 残局库** —— 不内置任何带估值/最佳着法的着法表;
  开局只有策略着法(前三手在 26 开局域内随机选型,trivial
  bestmove,见 `src/opening.rs` 与 `book/README.md`)
- **RIF 26 种开局规则**(索索夫等 swap 体系)—— 只做自由开局 + 禁手
  (26 开局会**认名**,但不强制开局区的落子限制;策略着法只约束
  AI 自己的前三手)
- **多线程** —— 单线程到底(wasm 里也最省体积)
- **NNUE / 学习型评估** —— 70 KB 预算装不下,评估保持手工窗分
- 历史路线图见 `docs/ROADMAP.md`(其中 P0/P1 的 PVS、期望窗口、TT 深度
  优先替换、VCT 已在 v0.2 落地;空着裁剪、LMR 调参等仍开放)

## License

MIT
