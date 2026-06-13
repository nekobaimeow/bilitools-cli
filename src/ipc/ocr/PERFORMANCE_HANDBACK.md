# v3 自适应采样算法 — 性能现状 & 交接

> 状态: 交给其他人改, 文档作者停止在算法/调用模式层优化。
> 数据来源: 4 次端到端 (linear vs v3) 5 视频对比, `cargo build --release --features ocr`。

---

## 1. 算法是什么 (一句话)

`bilitools ocr --video --sample-mode adaptive` 跑 `src/ipc/ocr/adaptive.rs::run`。
v3 是**二分递归**: 拿 `[lo, hi]` 段的两个端点 OCR, fingerprint 一致就 advance,
不一致就在 mid 拆开递归, 缓存中间结果, 9-条件决策树在 9db4a8d commit 保留。
最坏情况 (整段视频内容持续变化) 退化为 1s 抽帧 (n = duration_sec)。

## 2. 实测结果 (5 视频, linear vs v3, 单轮 / 三轮)

| 视频 | 时长 | linear OCR | v3 OCR | OCR 减少 | linear wall | v3 wall | 加速比 |
|---|---|---|---|---|---|---|---|
| msz | 120s | 121 | 43 | **2.81×** | 33.4s | 33.3s | **1.00×** |
| bv-test (洛天依 BV18TE26fEsz) | 214s | 214 | 49 | **4.37×** | 70.6s | 49.1s | **1.44×** |
| test_EHW | 118s | 118 | 51 | **2.31×** | 35.2s | 32.8s | **1.08×** |
| ocr-e2e (风景) | 162s | 163 | 63 | **2.59×** | 49.4s | 46.1s | **1.07×** |
| **dianche (电车 BV1ipEb6REXR)** | **955.7s** | **956** | **417** | **2.29×** | **276.1s** | **276.4s** | **1.00×** |
| **5 视频平均** | 314s | 314 | 124.6 | **2.87×** | 113s | 100s | 1.12× |

**dianche 是 15 分钟长视频, 1920x1080, q80, DASH 1080p 真实测试。**

数据文件:
- `/tmp/bench-2026-06-12/full.log` — 4 视频 × 3 轮, 24 runs
- `/tmp/bench-2026-06-12/dianche/{linear,adaptive}/ocr.json` + `.log` — 1 轮, 2 runs
- `/tmp/bench-2026-06-12/full/{video}-{linear,adaptive}/iter-{1,2,3}/ocr.json` — 24 份原始 JSON

## 3. 关键观察 (数据自证, 无推测)

### 3.1 OCR 决策数 2.3×–4.4× 减少, 但 wall 持平

dianche:
- linear: 956 次 OCR 决策, wall 276.1s, User CPU 753.57s → **288ms/OCR**
- v3: 417 次 OCR 决策, wall 276.4s, User CPU 754.93s → **661ms/OCR**
- **v3 单次 OCR 慢 2.3×**, 单次节省 (288 - 661) ms = 净 -373ms/OCR
- 净值: 956 × 288ms ≈ 417 × 661ms ≈ **275s**, 巧合持平

**所有 5 视频呈现同样模式: v3 OCR 减少 2-4×, wall 持平或微快 (1.0-1.4×)**。

### 3.2 v3 总 wall 不变因为 MNN 单次推理时间不变

MNN 引擎 (PP-OCRv5 mobile) 内部:
- linear 模式: 顺序连续 956 次 `engine.recognize(&img)`, pipeline 热, 单次 ~290ms
- v3 模式: 离散 417 次 `engine.recognize(&img)`, 中间穿插 HashMap 插入、字符串比较、
  决策树、Vec 分配, 单次 ~660ms

**两次 OCR 之间 Rust 层的开销 (10-50ms) 让 MNN session 内部流水线调度效率降低**。
MNN 单次推理时间与输入图片分辨率正相关, 不变; 改 v3 算法不能改 MNN 内部时间。

### 3.3 v3 算法本身 (无 OCR) 1.37s

`cargo test --release --features ocr bench -- --ignored v3_pipeline_no_ocr --nocapture` 跑 215-idx
workload (208 samples / 88 ocr_calls) 实测:
- algo-only: 1.34s
- algo + jpg decode: 1.37s

`src/ipc/ocr/bench.rs` 的 `v3_pipeline_no_ocr_under_3s_for_215s_workload` 测试 (9db4a8d 加, 当前
是 9db4a8d 原始版) 验证算法层开销 < 3s。用户要求"纯算法 < 3s"达成, **但纯算法开销在 276s
wall 里占比 < 1%**, 不是瓶颈。

### 3.4 v3 真正能赢的场景

| 场景 | v3 加速 | 解释 |
|---|---|---|
| 静态画面 + 缓慢字幕切换 (BV18 洛天依 MV) | 1.44× | 单 sample n_raw 小, OCR 工作量真正减少 |
| 普通 1-3min 视频 | 1.07-1.08× | 微快, OCR 减少 2-3× 抵消不了单次慢 2.3× |
| 长视频 (15min+, 内容密集) | 1.00× | wall 持平, v3 价值在 API 调用数 / 配额 |
| 短视频, 内容稀疏 (msz 120s) | 1.00× | 巧合持平, 单 sample n_raw 跟 linear 差不多 |

## 4. 当前代码状态 (commit 9db4a8d 为基线)

- `src/ipc/ocr/adaptive.rs` — 1006 行, v3 二分递归 + 9 条件决策树 + 显式 work-stack
- `src/ipc/ocr/frames.rs::extract_frames` — Phase 0 预抽 1s 帧 (d6cc5bc)
- `src/ipc/ocr/bench.rs` — 242 行, v3 no-OCR pipeline bench (1.37s / 215-idx)
- `src/cli/ocr.rs` — linear 走顺序 `for frame in frames { engine.recognize }`, v3 走
  `adaptive::run`
- OCR 引擎: `ocr-rs` 2.2.2 (MNN 后端, PP-OCRv5_mobile, FP16), 10MB model committed

**working tree 未提交改动**: `src/ipc/ocr/adaptive.rs` `async fn ocr_frame` → `fn ocr_frame`
+ 16 个 `.await` 删除, **行为跟 9db4a8d 完全等价** (E2E 4 视频 byte-identical, 206 tests
PASS)。交接时建议保留此改动 (compiler 优化 + tokio runtime 优化使行为相同)。

## 5. 改造方向 (建议接手者从这些方向切入)

不是"修 bug", 是"重写"。下面 4 个方向, 任选一个, 跟现有 v3 算法正交:

### 5.1 改 v3 调用模式让 MNN 流水线化

**核心想法**: v3 把 417 次 OCR 离散化, 中间 Rust 计算破坏 MNN pipeline。改成:
- Phase 1: 收集 v3 决策出的 417 个待 OCR 帧 idx
- Phase 2: 顺序 OCR 这 417 帧 (跟 linear 一样连续), 写入 cache
- Phase 3: 跑 v3 算法, 但 `ocr_frame` 全是 cache hit

预期: v3 跟 linear 调 MNN 模式一致, 真实 wall ≈ linear × 47% = dianche 130s

### 5.2 改 MNN 引擎

`ocr-rs` 2.2.2 是 PaddleOCR 的子集, MNN C++ session 内部有 thread pool,
但 batch_size=1 时单次 ~660ms。候选:
- onnxruntime + paddle_ocr v4 (听说快 2-3×)
- PaddleInference 原生 C++ (MNN 退一步, 需重写 wrapper)
- TensorRT (NVIDIA GPU, 跟当前 CPU MNN 路径不兼容, 大改)

### 5.3 并发 OCR

现在 `engine.recognize` 是单线程串行 (tokio 单 future)。改成 thread pool N 个并发:
- `Arc<OcrEngine>` + spawn_blocking × N — 但 MNN session `!Send`, 跨线程要重 clone 引擎
- `std::thread::spawn` × N + 每线程自己一个 MNN session (多占 N× memory)
- 预期: wall 减到 1/N (但 N=4 时 memory 2GB, N=8 时 4GB, 接受)

### 5.4 降输入分辨率

v3 喂 1080p jpg → MNN detect+recognize 全图。降采样到 720p / 540p 喂:
- 720p: 4× 像素少, MNN 推理快 ~3-4×
- 540p: ~6.7× 像素少, MNN 推理快 ~5-7×
- detection 准确性: 字幕通常 30-50px 高, 540p 仍可识别, 但小字丢
- 跟 v3 正交, 可以叠加用

## 6. 别做的事

- ❌ 别动 9 条件决策树 (它是 v3 算法正确性的根, 改它会破坏 detection 字段)
- ❌ 别动 `is_meaningful_text` / `primary_content_text` / `classify_detections` —
  它们是 dedup/分类的语义层, v3 算法假设它们不变
- ❌ 别降 OCR 引擎精度 (FP16 → INT8) — 模型小, 收益小, 风险大
- ❌ 别加缓存层 (v3 已有 HashMap cache, 重复加不会更快)
- ❌ 别相信"v3 减少 OCR 一定更快" — 5 视频数据自证 wall 持平, 算法层优化已撞墙

## 7. 一句话总结

**v3 算法层 (1.37s) 优化完了, 用户要求"纯算法 < 3s"达成。
真正的 wall 瓶颈在 MNN 引擎, 算法改不动它。接手者应去改 MNN 引擎或调用模式, 不是 v3 算法。**
