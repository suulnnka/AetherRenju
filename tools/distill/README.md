# L1-R 蒸馏训练管线(nnue-design.md M2,f32 阶段)

对 rapfi mix9svq 网络(无搜索)做纯网络蒸馏,训练 AetherRenju 的
L1-R 小型 NNUE(每格方向码本 + 分区头,h=16,约 3.9 万活跃参数)。
本阶段交付 **f32 权重**;量化(i8/i16)与 wasm 整合不在本管线内。

```
rapfi 引擎(教师)                rapfi Trainer(借用的训练器)          AetherRenju
┌─────────────────────┐   ┌──────────────────────────────┐   ┌──────────────────┐
│ pbrain-rapfi         │   │ model/aetherl1r.py  (学生模型) │   │ tools/distill/    │
│  distillgen 命令      ├──▶│ dataset/distillgen.py(数据集) ├──▶│  eval_l1r.py      │
│  教师自对弈采样+标注   │   │ train.py -c train_l1r_f32.yaml│   │  export_f32.py    │
└─────────────────────┘   └──────────────────────────────┘   │  check_f32_parity │
                                                                └──────────────────┘
```

## 0. 前置

- rapfi 引擎已构建:`rapfi/Rapfi/build/distill/pbrain-rapfi`
  (新增 `distillgen` 子命令,源码 `command/distillgen.cpp`)
- rapfi Trainer 依赖:`accelerate configargparse tensorboard lz4 pyyaml`
  (python = `/home/a/miniconda3/bin/python3`,torch 已带 CUDA)
- 教师权重用仓库自带 `rapfi/Networks/mix9svq/`:
  - 无禁手:`mix9svqfreestyle_bsmix.bin.lz4`(黑白共用)
  - 有禁手:`mix9svqrenju_bs15_{black,white}.bin.lz4`(黑白分开)

## 1. 生成蒸馏数据(`rapfi distillgen`)

教师网络纯策略采样自对弈(**零搜索**):每局面记录
`(盘面, 行棋方, 规则, 教师 WDL 概率, 教师策略分布, 禁手位图, 终局结果)`。

```bash
cd rapfi/Rapfi
NET=../Networks/mix9svq
D=../../AetherRenju/tools/distill/data

# 无禁手(平衡局 + 自然局,60/40 混合)
./build/distill/pbrain-rapfi distillgen -o $D/fs_bal_train.bin --rule freestyle \
  --weight-file $NET/mix9svqfreestyle_bsmix.bin.lz4 \
  -n 6000 --seed 1001 --temp 1.0 --opening-temp 1.2 --eps 0.05 --balance-thresh 0.7 --no-message
./build/distill/pbrain-rapfi distillgen -o $D/fs_nat_train.bin --rule freestyle \
  --weight-file $NET/mix9svqfreestyle_bsmix.bin.lz4 \
  -n 8000 --seed 1002 --temp 1.0 --opening-temp 1.2 --eps 0.05 --no-message

# 有禁手(renju,黑白教师分网)
./build/distill/pbrain-rapfi distillgen -o $D/rj_bal_train.bin --rule renju \
  --weight-file-black $NET/mix9svqrenju_bs15_black.bin.lz4 \
  --weight-file-white $NET/mix9svqrenju_bs15_white.bin.lz4 \
  -n 6000 --seed 3001 --temp 1.0 --opening-temp 1.2 --eps 0.05 --balance-thresh 0.7 --no-message
./build/distill/pbrain-rapfi distillgen -o $D/rj_nat_train.bin --rule renju \
  --weight-file-black $NET/mix9svqrenju_bs15_black.bin.lz4 \
  --weight-file-white $NET/mix9svqrenju_bs15_white.bin.lz4 \
  -n 8000 --seed 3002 --temp 1.0 --opening-temp 1.2 --eps 0.05 --no-message

# 验证集(独立种子,规模约为训练集 5%)
#   *_bal_val.bin: -n 300 --seed 2001/4001 + --balance-thresh 0.7
#   *_nat_val.bin: -n 400 --seed 2002/4002(同上参数)
```

要点:

- **平衡采样** `--balance-thresh 0.7`:试走后教师 |WLR| 超阈值则重采样
  (最多 4 次),平均手数 14→26,中局均衡局面占比 18%→45%;
- **自然局**(无平衡)保留真实收官分布,两者 60/40 混合;
- 生成速度约 3 万局面/秒,88 万训练局面 < 1 分钟;
- 当前数据:无禁 40.5 万 + 有禁 47.5 万 = 88.1 万训练 / 4.3 万验证
  (约为学生网络活跃参数量的 23 倍,超过设计文档 10× 红线)。

文件格式(`DISTILL1`,小端):头 8B 魔数 + u32 版本 + u8 盘面 + u8 规则 +
u32 局数 + u64 局面数;每局 u16 局面数 + u8 结果;每局面
`u8 stm + u8 ply + u8 盘面[225](0空/1黑/2白) + u8 禁手位图[29] +
f32 教师 WDL[3](行棋方视角,胜/负/和) + f16 策略[225]`。

## 2. 训练(借用 rapfi Trainer)

学生模型与数据集作为 Trainer 插件(注册名 `aetherl1r` / `distillgen_binary`),
训练主循环、断点、TensorBoard、验证全部复用 rapfi 训练器:

```bash
cd rapfi/Trainer
accelerate launch train.py -c ../../AetherRenju/tools/distill/train_l1r_f32.yaml
# 断点续训:accelerate launch train.py -c <rundir>/run_config.yaml
```

关键配置(见 yaml):`loss_type MSE+NONE`(值蒸馏,sigmoid 胜率 MSE vs
教师 (胜−负+1)/2)、batch 2048、AdamW lr 1.5e-3 + step 衰减、40K 迭代
(约 93 epoch)、8 对称增广、每 2500 迭代验证。

学生结构严格按 nnue-design.md §4(h=16 定型配置):
中心 7 格窗码本(E_hv 横竖共享 / E_di 双斜共享,词表 3^7,活跃 919 行)+
己子 PSQ + 逐格 SCReLU + 3×3 分区池化 + 146→16→1 头(行棋方与规则作为
额外 2 维输入,**一张网同时服务有禁/无禁**)+ tempo。
全表 79,554 参数;活跃行 38,978 参数(与设计文档预算一致)。

实现要点:

- **死段零贡献**:活跃 = 存在单色 5 子窗(919/2187,已数值验证与文档
  一致)。训练时死 id 直接乘 0 掩码(部署时不存储死行),否则死行会
  被真实盘面训出权重、导出时有损;
- **F.embedding 而非高级索引**:真实盘面 id 高度集中(空窗为主),
  高级索引反向的原子散射在热门行上碰撞,稀疏数据每步 >200ms;
  F.embedding 反向是排序分段规约,~10ms/步(32× 加速);
- **num_worker=0**:Python 3.14 的 forkserver 会把整个数据集 pickle
  进每个 worker(每份 ~1.5GB);单进程加载已 ~80K 样本/秒,够 GPU 吃。

## 3. 评估与导出

```bash
cd rapfi/Trainer
# 验证集指标(胜率 MSE / WLR MAE / 符号一致率,按规则与手数分组)
python ../../AetherRenju/tools/distill/eval_l1r.py \
  -c ../../AetherRenju/tools/distill/runs/l1r_f32_v1/run_config.yaml \
  -p ../../AetherRenju/tools/distill/runs/l1r_f32_v1/ckpt_aetherl1r_h16_*

# 导出 f32 权重(活跃行压缩:2187 → 919 行 + id→行映射)
python ../../AetherRenju/tools/distill/export_f32.py \
  -c .../run_config.yaml -p .../ckpt_aetherl1r_h16_XXXXXXX \
  -o ../../AetherRenju/weights/l1r_f32.bin --meta-out ../../AetherRenju/weights/l1r_f32.json \
  --note "distilled from mix9svq, no search"

# 纯 numpy 复算导出文件,与 torch checkpoint 对拍
python ../../AetherRenju/tools/distill/check_f32_parity.py \
  -b ../../AetherRenju/weights/l1r_f32.bin -c .../run_config.yaml -p .../ckpt_...
```

导出布局:魔数 `ALR1` + 版本 + 盘面/窗长/宽度/活跃行数 + u16 活跃 id 表 +
f32 数据块(E_hv / E_di / PSQ / 头 / tempo)+ CRC32。量化阶段(i8 嵌入、
i16 头、scale)在此基础上另行扩展。

## 4. 已知边界(后续阶段处理)

- **量化**:i8/i16 与 per-channel scale 未做,导出为 f32;
- **整合**:wasm 侧增量 make/unmake(SEGID/SEGOFF/DEAD 基础设施)未动,
  本管线不触碰 `src/`;
- **数据分布**:策略采样 + 平衡拒绝使对局偏"势均力敌",收官极端局面
  靠自然局 40% 兜底;若实测收官估值差,可调 `--balance-thresh` 再生成;
- **验证口径**:与教师的一致性 ≠ 棋力;最终验收按设计文档 §9 三闸门
  (体积/速度/棋力)在整合阶段跑。


## 5. 量化 + Rice + 引擎整合(v0.3,本次交付)

### 5.1 量化(QAT,部署精确图)

`model/aetherl1rq.py`:与部署端逐位一致的伪量化图(STE),常量
`QE=64 CF=1024 QW1=128 S1=13 SH=64 CH=1024 QW2=256 S2=12 SO=256`
(由 f32 网实测标定;i8 嵌入/PSQ + i16 头 + i32 累加,i64 点积)。
关键教训:

- **热行量子化噪声**:空窗 id=0 等热行的微小嵌入值(~0.003)被 QE
  抹零会造成系统偏移,靠 QAT 微调让网络自适应(零训练 MSE 0.11 →
  训后 0.042);
- **f64 点积**:dot 量级 ~10⁹,f32 矩阵乘的尾数(24 位)不够,
  QAT 必须用 f64 镜像引擎的 i64 点积。

### 5.2 Rice 编码(ALR2,参考 AetherChess3 LER1)

`export_int_rice.py`:zigzag + MSB-first Golomb-Rice;i8 段(e_hv/
e_di/psq)按 256 值分块自适应 k,i16 段(fc1w/fc1b/fc2w/fc2b+tempo)
全局 k;头部携带量化常数与 cp 映射参数,尾 CRC32,内置往返自检。
`src/nnue.rs` init 时解码进静态区(解码器 ~150 行,不占运行时)。

### 5.3 引擎整合(NNUE-only)

`src/nnue.rs`:SEGID[900] 窗 id 增量(±v·3^k 自逆)+ 行差分 F/Z
维护(死行跳过,实测快于 SIMD 版)+ i64x2 SIMD 头(wasm128)。
`src/lib.rs`:`evaluate(side, mode)` 只读 NNUE,**WSC 窗分评估路径
已删除**;wb/ww/cnt 与 point_score/tier 保留为搜索排序设施(设计
文档 §6 口径)。cp 映射 = 饱和有理式 `cp = 9000·a/(a+1390)`(a 为
logit×256;纯整数、严格反对称、两端饱和保排序梯度)。

对拍:`tests/nnue_fixtures.txt`(2000 局面,QAT 模型逐位 cp)+
`nnue_incremental_matches_refresh`(随机对局行进中周期全量对拍)
双保险,`cargo test` 全绿。

### 5.4 闸门实测(本机,v3 网)

| 闸门 | 标准 | 实测 | 结论 |
|---|---|---|---|
| 体积 | wasm gzip ≤ 70KB | **55.8 KB** | ✅(余 14KB) |
| 回归 | cargo test | 全绿 | ✅ |
| NPS | ≥ v0.2 的 50% | 212–259K(v0.2 ≈930K 的 23–28%) | ❌ 见下 |
| 棋力 | vs v0.2 ≥55% | 无禁 3:26 / 有禁 4:22(随机开局 30 局/规则,10 万节点) | ❌ 见下 |

### 5.5 棋力现状与根因(重要)

教师 mix9svq 本身说无禁空盘黑 ~86%——蒸馏网在这点上**没错**。输棋
根因有两层,均有实证:

1. **树内幻觉(分布漂移)**:引擎搜索自造的局面偏离教师自对弈分布,
   网对"己方投机攻击线"给出 logit 8+ 的幻觉高分(静态评估 −1806 的
   局面,搜索 4 层后报 +5498),于是无视对手正在成型的 VCT 杀;
   v0.2 靠根 VCT(深度 12、3 千节点)先一步找到强制杀终结。
2. **威胁滞后**:局面翻转点(教师 26→30 手间从黑 94% 翻到白 67%)
   学生网仍报黑优——决定性转折局面在数据里占比不足(v2 已把自然局
   提到 70%,v3 再加教师策略蒸馏辅助头,对局从 1:49 / 5:44 改善到
   3:26 / 4:22,但仍不敌)。

NPS 缺口:头部 146×16 次乘加是 qsearch 叶子硬成本(SIMD 后占 ~40%),
make 增量 ~1.5K ops;opt-level 2 + 行差分 + SIMD 头已从 127K 提到
212–259K,再往上需削减头宽或惰性评估。

### 5.6 下一步建议(按预期收益排序)

1. **DAgger 循环**:用当前 NNUE 引擎自对弈生成局面、教师标注、重训
   (工具链已齐:distillgen 3 万局面/秒 + 一键训练),直接消灭树内
   分布漂移——这是 0:40 类惨败的对症药;
2. 策略头部署化:把教师策略蒸馏的 225 路头做小(如每区 16 维)随网
   导出,替代 point_score 排序;
3. 搜索侧:对手活三时不 stand-pat(qsearch 扩展,纯搜索设施);
4. NPS:头部输入截断(小权重剪枝)或 h=12 备胎。

### 5.7 与原版 JS 引擎(git HEAD~1 src/engine.js)对比

等节点预算(10 万节点/手)、随机开局、双规则各 30 局:

| 规则 | 原版 JS(手调 WSC) | NNUE 新引擎 |
|---|---|---|
| 无禁 | **29** | 1 |
| 有禁 | **25** | 4(和 1) |

体积:原版 engine.js 30.2 KB(gzip 10.7 KB)→ 新 wasm 104 KB
(gzip 55.3 KB,其中 Rice 权重 33.6 KB)。原版 JS 与 v0.2 HCE 同源
(手调五元窗),结论一致:**当前 NNUE 评估的实战强度不敌手调系**,
根因与对策见 §5.5/§5.6(DAgger 循环是对症药)。


## 6. v0.4:评估切换为线形分类 HCE(NNUE 已移除)

NNUE 小模型(39K 参数)实战不敌手调系(§5.5/§5.7)后,评估改走
「线形分类 + 小权重表」路线,**移除 NNUE**,引擎回到单评估器
(src/eval.rs,自研实现):

- **体系**:每格每方向 16 类线形(窗口试放 + 记忆化分类;有禁黑方
  含禁手语义)→ 四向组合取格码(3876)聚合威胁级(14)
  → EVALS + THREAT 权重表 → 评估 = 两拍平滑 + 11 位威胁掩码查表,
  钳 ±6000。增量:落/提子重算 ±H 线内空格(H=4 无禁/5 有禁),
  差分更新,走子格状态冻结(LIFO 撤销序下自然恢复)。
- **权重**(src/eval_tables.bin,AHCE 格式):EVALS/THREAT 六表,
  起点为 CC0 发布的经典权重(rapfi Networks/classical/model220723,
  Networks 目录整体 CC0),Texel 式调参见 §7。
- **对拍**:1600 局面(双规则)与外部真值逐位一致
  (`tests/eval_fixtures.txt` + `eval_matches_fixtures`);增量 vs 全量
  一致性 fuzz;逐格线形 225/225 一致(examples/dump_cells.rs)。
- **工具链**:`rapfi evaldump`(C++ 真值,本地构建)、
  `tools/distill/gen_hce_fixtures.py`(夹具生成)、
  `tools/distill/tune_hce.py`(Texel 调参)。

### v0.4 闸门实测

| 闸门 | 标准 | 实测 | 结论 |
|---|---|---|---|
| 体积 | wasm gzip ≤ 70KB | **43.5 KB** | ✅ |
| 回归 | cargo test | 全绿(含 1600 局面 rapfi 逐位对拍) | ✅ |
| NPS | ≥ v0.2 的 50% | 250–453K(v0.2 ≈930K 的 27–49%) | ⚠️ 大师档未达 |
| 棋力 | vs v0.2 ≥55% | **无禁 17:13 / 有禁 19:9(合计 60%)** | ✅ |
| 棋力 | vs 原版 JS | 无禁 18:12 / 有禁 22:8(67%) | ✅ |

对局条件:随机开局、等节点(10 万/手)、双规则各 30 局。
NNUE 相关代码(nnue.rs/rice/夹具)已删除;蒸馏管线(tools/distill)
保留,后续若重训更强网络可再接回。


## 7. HCE 权重 Texel 调参(tune_hce.py)

线性模型逐权重可导:评估 = EVALS × 两拍 pcode 直方图 + THREAT[mask],
sigmoid(v/250) 对教师 WDL 概率做 MSE,锚定正则(λ=1e-5)拉向 CC0
原版权重,量化即四舍五入到 i16(±1400 钳制,掩码 0 威胁项钉原值)。

- **数据**:§1 的 distillgen 数据(训练 91.6 万 / 验证 4.2 万局面,
  教师 WDL 标签);特征由 `examples/dump_features` 从引擎导出
  (两拍稀疏直方图 + 11 位掩码;直方图由表格式定义,与实现无关)。
- **命令**(seed 固定可复现):
  `python tune_hce.py --train /tmp/feat2_train --val /tmp/feat2_val \
   --model .../model220723.bin --out-tuned-model /tmp/model_tuned4.bin \
   --out-tables src/eval_tables.bin --seed 42 --epochs 16`
- **验证集 winrate-MSE**:

| 权重 | val MSE | 备注 |
|---|---|---|
| CC0 原版(model220723) | 0.15301 | 基线 |
| 上一轮调参 | 0.11877 | 4 epoch 量级 |
| **本轮 16 epoch(seed 42)** | **0.11354**(量化 0.11375) | 已安装 |

- **强度 A/B**(等节点 10 万/手、随机开局、双规则各 50 局,
  新表 vs 上一轮):无禁 **33:17**、有禁 **32:16**(和 2)。
- **回归链**:装表后用 `gen_hce_fixtures.py <tuned-model>` 重生成
  1600 夹具(rapfi evaldump + 调参回写模型 = 独立真值),
  `eval_matches_fixtures` 必须全过 —— 引擎与真值在调参后仍逐位一致。
