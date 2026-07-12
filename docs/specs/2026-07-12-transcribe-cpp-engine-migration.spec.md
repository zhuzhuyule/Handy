---
name: "转录引擎迁移到 transcribe.cpp（B 方案）"
tags: [engine, transcribe-cpp, models, migration, asr]
depends_on: []
estimate: "多周（分 6 阶段）"
---

## 意图

"将本地转录引擎从 `transcribe-rs`（Rust/ONNX，按家族各一套引擎）**彻底替换**为上游 Handy 0.9.x 采用的 `transcribe.cpp`（C/C++ 的 ggml + 统一 GGUF 格式，单引擎覆盖 16 个家族 / 60+ 模型，原生流式），解决我们可用模型有限的问题。先在 macOS 跑通、先批处理后流式，把模型目录外置并接入 HuggingFace 下载。"

背景与根因见 `docs/`（会话调研）：我们"模型有限"不是引擎跑不动，而是目录硬编码 + per-family ONNX 架构扩展成本高。上游用单一 ggml/GGUF 引擎统一了所有家族，并把目录抽成脚本自动生成的 `catalog.json`。本方案采纳上游做法。

## 约束

- **平台**：Phase 0-3 仅 macOS（metal 静态链接，最简单）；Windows/Linux 作为 Phase 4。
- **彻底替换**：移除 `transcribe-rs` 依赖及 `transcription.rs` 中全部 ONNX per-family 引擎变体；`transcribe-rs` 在我们代码里仅 `transcription.rs` 使用（9 处），爆炸半径小。
- **不得破坏**下列独立子系统（它们不依赖 transcribe-rs）：
  - 在线 ASR（`async-openai` staggered gitee/groq，`online_asr.rs` / `transcribe.rs`）；
  - VAD（`vad-rs`）与音频采集/重采样（cpal / rubato）；
  - 后处理管线、review 窗口、overlay、热词系统。
- **实时 worker**：Phase 1 起改用 transcribe.cpp 批处理（替换 `try_transcribe_raw` 的 transcribe-rs 调用）；原生流式留到 Phase 3。
- **标点模型**：`transcribe_rs::punct::PunctModel`（transcription.rs:126）随替换一并移除——依赖 GGUF 模型原生标点（whisper/sensevoice/parakeet 等原生带标点）。若某模型输出裸文本，暂时接受无标点，Phase 3 再评估。
- 遵守 CLAUDE.md runtime rules（协调器线程不 block_on、非 async 上下文不 tokio::spawn 等）。
- 每阶段收口：编译零 warning、现有测试全绿、目标平台手动验证通过。
- 现有用户已下载的 ONNX 模型将失效 → 必须提供迁移路径（识别 legacy 模型引用，引导重下 GGUF，不崩溃）。

## 已定决策

1. **引擎 = transcribe.cpp，经 crates.io `transcribe-cpp` 安全封装（FFI，非子进程）**。版本 pin 到 Handy 0.9.1 所用版本（Phase 0 读其 `src-tauri/Cargo.toml` 确认，#1589/#1634 有 bump）。
2. **彻底替换 transcribe-rs**（用户拍板，比上游更激进——上游是双引擎并存）。`EngineType` 收敛为单一 `TranscribeCpp`，删除 `Whisper/Parakeet/Moonshine/MoonshineStreaming/SenseVoice/Paraformer/ZipformerTransducer/ZipformerCtc` 等 ONNX 变体与对应 `LoadedEngine` 变体、导入、match 分支。
3. **标点**：弃用独立 punct 模型，依赖 GGUF 原生标点（见约束）。
4. **模型目录外置**：移植上游 `catalog.json` 结构 + `catalog/mod.rs` 归一化为描述符；下载改用 `hf-hub` 进共享 HF cache；初期直接复用上游 `handy-computer/*-gguf` HF 仓库。
5. **macOS 链接**：metal 静态链接；`build.rs` stage transcribe.cpp 运行库。采纳 #1510（仅 Command Line Tools 的 macOS 构建回退）。
6. **批处理优先**：Phase 0-2 仅批处理，实时 worker 走 transcribe.cpp 批处理；原生流式 Phase 3。
7. **从一开始吸收 0.9.1 的 transcribe.cpp 用法教训**：
   - #1602 所有模型统一 auto timestamps；
   - #1603 whisper run extension 按模型架构门控，而非 `Feature::InitialPrompt`；
   - #1597 非流式模型在 Live overlay 显示 "Processing"。

## 阶段划分

| 阶段             | 目标                                                                                                                       | 平台      | 主要产出                                      |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------- | --------- | --------------------------------------------- |
| **P0 spike**     | 去风险：`transcribe-cpp` 依赖能构建、加载 1 个 GGUF、转录 1 个 WAV 文件正确                                                | macOS     | 一次性验证测试 + build.rs 运行库 staging 打通 |
| **P1 引擎接入**  | `EngineType::TranscribeCpp` 打通 load + 批量转写，走通"录音→转录→后处理"全链路；实时 worker 改走 batch；移除 transcribe-rs | macOS     | 可日常使用的单引擎批处理版                    |
| **P2 目录+模型** | 外置 `catalog.json` + `catalog/mod.rs`，`hf-hub` 下载，UI 暴露完整模型列表；legacy 模型引用迁移引导                        | macOS     | 65 模型可下载可选                             |
| **P3 原生流式**  | transcribe.cpp `stream_begin/feed/finalize` 替换滑窗实时 worker，与 overlay 实时显示、在线 ASR 协调                        | macOS     | 原生流式实时字幕                              |
| **P4 跨平台**    | Windows/Linux 动态后端 + dlopen ggml + 运行库/VC++/onnxruntime staging（吸收 #1577/#1187/#1621）                           | Win/Linux | 三平台可发布                                  |
| **P5 收尾**      | 清理 legacy 残留、文档、模型迁移体验打磨                                                                                   | 全        | 迁移完成                                      |

## 边界

### 允许修改

- `src-tauri/Cargo.toml`（换依赖）、`src-tauri/build.rs`（运行库 staging）
- `src-tauri/src/managers/transcription.rs`（引擎核心，替换重灾区）
- `src-tauri/src/managers/model.rs`（catalog + EngineType + 下载）
- `src-tauri/src/catalog/`（新增模块，移植自上游）
- `src-tauri/src/actions/transcribe.rs`（实时 worker 调用点 + overlay 状态）
- 前端 `src/lib/types.ts`（EngineType schema）、`src/components/settings/asr-models/*`（分类/展示）
- `src-tauri/src/commands/models.rs`（下载命令，如需）

### 禁止

- 不动在线 ASR 逻辑（`online_asr.rs` 的 provider/proxy/staggered）——与本迁移正交。
- 不动后处理管线、review 窗口、热词、overlay 内存泄漏修复。
- 不在本分支夹带 0.9.1 的独立快赢（见排除范围）。

## 排除范围

- **本迁移不含**的 0.9.1 独立改进（另行单独评估/单独分支）：
  - #1310 后处理提示词 prompt-injection 防御（安全，独立高价值）
  - #1344 录音间重置重采样器防音频串扰
  - #1631 设置解析失败抢救
  - Light/Dark/System 外观选择器、各语言翻译、tray 修复等 UI/i18n。
- 原生流式（P3）、Windows/Linux（P4）不在 P0-P2 范围。
- 自建 HF 模型托管（初期复用上游仓库）。

## 验收场景

### 1. p0_spike_transcribes_file（Happy path）

- **Given**: macOS 开发机，`transcribe-cpp` 依赖已加入，一个已知 GGUF 模型 + 一段已知 WAV
- **When**: 运行一次性验证测试加载模型并转录该 WAV
- **Then**: 输出文本与预期一致，构建产物包含 staged 的 transcribe.cpp 运行库，进程不崩

### 2. p1_hotkey_batch_transcription（Happy path）

- **Given**: P1 完成，选中一个 GGUF 模型
- **When**: 按快捷键录音说话并松开
- **Then**: 经 transcribe.cpp 批处理得到文本，overlay 状态流转正常（含 #1597 非流式 "Processing"），后处理/插入链路不变

### 3. legacy_model_ref_migration（Error/edge path）

- **Given**: 用户设置里 `selected_model` 指向一个已删除的旧 ONNX 模型
- **When**: 启动/开始录音
- **Then**: 不崩溃；提示该模型已不受支持并引导下载 GGUF 替代，或回退到默认 GGUF 模型

### 4. transcribe_rs_fully_removed（Constraint check）

- **Given**: P1 完成
- **When**: 全仓 grep `transcribe_rs`
- **Then**: 零引用；`Cargo.toml` 无 transcribe-rs 依赖；`cargo build` 通过、`cargo test` 全绿、零 warning

### 5. online_asr_unaffected（Regression guard）

- **Given**: 在线 ASR（gitee/groq staggered）配置不变
- **When**: 在线模式录音
- **Then**: 在线转录与 fallback 行为与迁移前一致（该路径不经 transcribe-rs/transcribe.cpp 本地引擎）

### 6. realtime_preview_via_batch（Happy path, P1）

- **Given**: 实时预览开启
- **When**: 录音说话
- **Then**: 实时 worker 通过 transcribe.cpp 批处理产出 `realtime-partial`，最终结果仍由完整音频批处理决定（解耦不变）

## 实施偏差

> 各阶段完成后回填。

| 原计划                                                 | 实际实现                                                                                                                                                                  | 原因                                        |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| pin `transcribe-cpp = 0.1.2`                           | `^0.1.2` 解析到 **0.1.3**（当前 crates.io 最新 0.1.x），构建/运行均通过                                                                                                   | 语义化版本兼容，0.1.3 是 crate 仓库当前版本 |
| 假设上游 v0.9.x 已全量 GGUF、transcribe-rs 为纯 legacy | 核实 v0.9.1 实为**双引擎**：Whisper 家族走 transcribe-cpp(GGUF)，Parakeet/Moonshine/SenseVoice/GigaAM/Canary 仍走 transcribe-rs(ONNX)。我们仍按"彻底替换"走（比上游激进） | 上游是渐进迁移；我们一步到位更干净          |
| —                                                      | catalog.json(65 模型) **不含 paraformer / zipformer**；彻底替换会丢这两个，换来 SenseVoice/Qwen3-ASR/Fun-ASR 等 GGUF                                                      | 中文能力净增（自主判断可接受，见会话）      |
| —                                                      | **P0 spike 已通过**：transcribe-cpp 0.1.3(CMake+ggml+Metal)在 macOS 树内构建成功；`examples/tc_spike.rs` 加载 whisper-tiny GGUF 转录 jfk.wav 输出正确、Metal 加速         | 去风险验证，B 方案技术可行性确认            |
