---
name: "Polish pipeline 双层超时"
tags: [post-processing, fallback, latency, reliability]
depends_on:
  - "fallback::execute_with_fallback (Serial/Race/Staggered)"
  - "actions/post_process/pipeline.rs::unified_post_process"
  - "actions/post_process/extensions.rs (multi-model HTTP path)"
estimate: "1 day"
---

## 意图

"为 polish pipeline 加两层 timeout：每次 LLM 调用 5 秒上限，整条 pipeline 10 秒硬顶。单次超时让 fallback 策略继续推进；总超时取消所有 in-flight LLM 任务，返回 ASR 原文（PassThrough）插入。"

解决的问题：当前 polish 链路的唯一 timeout 是 reqwest 客户端的 60 秒——对短文本语音输入的 UX 太宽松。一次 provider 排队过载（HTTP 429）就可能导致 race fallback 等待数十秒，用户体感"卡死"。spec 让语音转录在最坏 10 秒内一定有结果可粘贴。

## 约束

- 单次 LLM 调用超时 = **5 秒**。整条 polish pipeline 超时 = **10 秒**。两个都是常量，不暴露 UI。
- 5 秒覆盖**所有** LLM 调用路径：`core.rs` 的单链 fallback (Serial / Race / Staggered) **以及** `extensions.rs` 的 multi-model 候选。
- 10 秒只在 polish 阶段（实际 LLM 工作）计时——history hit / intent routing 等毫秒级步骤不计入。
- 10 秒兜底必须是 `PipelineResult::PassThrough { text: <原 ASR>, intent_token_count: None }`。**不弹 toast**——与现有 PassThrough 行为一致。
- 5 秒触发不需要用户感知；fallback 策略继续，正常路径下用户只会感觉模型"换了一个"。
- reqwest 60 秒客户端 timeout **保留不动**——作为最后兜底，且与单次 5 秒不冲突（5 秒先触发）。
- 不修改 `fallback.rs` —— Serial / Race / Staggered 三种策略的代码一行不动。注入靠包装 `execute_fn`。
- 不修改 `PipelineResult` 枚举 —— 直接复用现有 `PassThrough` 变体。
- 不改 LLM HTTP 客户端实现。
- 修改前提交前消除所有 warning。

## 已定决策

- **两层 timeout 而非单层。** 用户明确要求"每个模型 5 秒"+"总 10 秒"两个约束。仅 10 秒 pipeline 则单次悬挂模型可耗尽全部预算、永远轮不到 fallback；仅 5 秒单次则总时长无硬顶（Serial 链 5+5 已经够，但极端 stagger / Race+多 retry 可叠加）。两个一起才同时满足两条约束。

- **常量而非 settings。** 5/10 这两个数对绝大多数用户都合理。提早暴露 UI 选项违反 YAGNI，且会让代码多一层 config 读取。等到真有人提"我有理由要 8/15"时再加。

- **5 秒包装的注入点 = `core.rs` 调 `execute_with_fallback` 前 + `extensions.rs` 调 HTTP 前。** 不在 `fallback.rs` 内部加——保持策略代码纯净。包装方式：用 `tokio::time::timeout(Duration::from_secs(5), execute_fn(id))`，超时映射为 `Err("Model call exceeded 5s timeout")` 字符串，让 fallback 把它当作普通失败处理。

- **10 秒包装的注入点 = `pipeline.rs::unified_post_process` 主体。** 把现有函数体重命名为 `unified_post_process_inner`，公开入口包 `tokio::time::timeout`。超时分支返回 `PassThrough { text: original_text.to_string(), intent_token_count: None }`。

- **PassThrough 兜底语义统一适用所有 polish 场景。** LitePolish / FullPolish / multi-model / Skill 超时均回到 ASR 原文。不为 Skill 加特殊错误提示——Skill 失败本就经常 fallback 到原文（spec §约束已锁定）。

- **`tokio::time::timeout` 的 future drop 即取消语义足够。** reqwest 的 future 被 drop 时会取消底层 TCP 请求。不需要额外 cleanup 逻辑。

- **超时事件用 `log::warn!` 标记，不写新 DB 列。** 现有 `llm_call_log` / `pipeline_decisions` 表足够诊断；超时表现为 `duration_ms ≈ 5000` + error string 含 "exceeded 5s timeout"。日志 grep 可定位。

## 边界

### 允许修改

- `src-tauri/src/actions/post_process/core.rs`：
  - 加 `const PER_CALL_TIMEOUT_SECS: u64 = 5;`
  - 在调 `fallback::execute_with_fallback(chain, execute_fn)` 处，先用 `tokio::time::timeout` 包装 `execute_fn`
- `src-tauri/src/actions/post_process/extensions.rs`：
  - multi-model 内的 HTTP 调用同样用 `tokio::time::timeout` 包 5 秒
- `src-tauri/src/actions/post_process/pipeline.rs`：
  - 加 `const PIPELINE_TIMEOUT_SECS: u64 = 10;`
  - `unified_post_process` 函数体抽到 `unified_post_process_inner`
  - 公开的 `unified_post_process` 包 `tokio::time::timeout(10s, inner)`，超时返回 PassThrough

### 禁止

- 修改 `src-tauri/src/fallback.rs`（策略代码不动）
- 修改 `PipelineResult` 枚举（复用现有 PassThrough 变体）
- 修改 LLM HTTP 客户端 reqwest 配置（保持 60s 兜底）
- 暴露 5s / 10s 到 settings UI
- 加新的 Tauri command / event 通知前端"超时了"

## 排除范围

- 配置化：阈值不开 UI 设置项。
- 用户提示：超时不弹 toast / 不写日志面板。
- 历史回填：现有记录不受影响。
- 重试策略变化：reqwest 60s + 现有 retry 不动。
- 不在 ASR 阶段引入 timeout（spec 仅限 polish 后处理）。
- 不在 hotword / intent 路由 / history 查找等毫秒级步骤上加 timeout。
- 不为单次 5s 超时添加单独的 metrics 字段——`llm_call_log.duration_ms` 已可观察。

## 验收场景

### 1. happy_path_short_text_no_timeout

- **Given**: 用户口述 30 字短句，配置 LitePolish 单模型 prompt
- **When**: LLM 调用在 1.5 秒内返回成功
- **Then**:
  - polish 正常返回，PipelineResult::SingleModel
  - 既未触发 5 秒也未触发 10 秒
  - 用户拿到 polish 后文本

### 2. happy_path_long_text_within_5s

- **Given**: 用户口述 400 字长文本，LLM 主模型 4 秒返回
- **When**: 4 秒 < 5 秒阈值
- **Then**:
  - 未触发任何 timeout
  - polish 正常完成

### 3. per_call_timeout_triggers_fallback_serial

- **Given**: Serial 链配置 primary=A, fallback=B
- **When**:
  1. A 调用 5.1 秒未返回 → `tokio::time::timeout` 触发
  2. fallback.rs::execute_serial 收到 `Err("Model call exceeded 5s timeout")`
  3. 开始调 B
  4. B 在 1 秒内成功
- **Then**:
  - 总用时 ≈ 6 秒
  - PipelineResult::SingleModel（用 B 的结果）
  - llm_call_log 中 A 行 `duration_ms ≈ 5000`、error 含 "exceeded 5s"；B 行正常
  - log warn 出现 "[Pipeline] model A exceeded 5s"

### 4. per_call_timeout_triggers_fallback_race

- **Given**: Race 配置 primary=A, fallback=B
- **When**:
  1. A 和 B 在 t=0 并发启动
  2. 两者都在 5 秒时被 timeout 取消（均超 5 秒）
  3. fallback.rs::execute_race 在 5 秒时两个 future 都返回 Err
- **Then**:
  - Race 在 ~5 秒时返回最终 Err
  - 这个 Err 向上传到 unified_post_process_inner
  - inner 的 polish 失败（不被 10s 全局 timeout 触发，因为已经在 5s 时拿到 Err）
  - pipeline 决定如何处理（现有逻辑：可能直接返回失败 SingleModel，由上层 fallback 到 PassThrough）
  - 总用时 ≈ 5 秒

### 5. pipeline_timeout_serial_double_timeout

- **Given**: Serial 配置 primary=A, fallback=B；A 和 B 都会卡 6 秒
- **When**:
  1. A 调用 5 秒被取消 → Err
  2. B 调用启动
  3. B 调用 5 秒被取消 → Err
  4. fallback.rs 总耗时 ≈ 10 秒，返回 Err
  5. 但是 unified_post_process 外层 timeout 在恰好 10 秒时也触发
- **Then**:
  - 总用时 ≈ 10 秒（要么 fallback 自己失败、要么 pipeline timeout 触发——两者时间几乎重合）
  - 最终返回 `PipelineResult::PassThrough { text: <原 ASR>, intent_token_count: None }`
  - log warn 出现 "[Pipeline] exceeded 10s ceiling, falling back to PassThrough"
  - 用户拿到 ASR 原文，无 toast

### 6. pipeline_timeout_staggered

- **Given**: Staggered 配置 primary=A, fallback=B（STAGGERED_DELAY 2s）；A 卡 5 秒、B 卡 5 秒
- **When**:
  1. t=0 A 启动
  2. t=2 B 启动（A 还在跑）
  3. t=5 A 被 timeout 取消 → Err
  4. t=7 B 被 timeout 取消 → Err
  5. fallback.rs 在 ~7 秒返回 Err
  6. 10 秒上限未触发
- **Then**:
  - 总用时 ≈ 7 秒
  - 现有上层 fallback 逻辑接管（取决于调用者怎么处理 polish 失败；最常见是返回 SingleModel error=true 然后由 transcribe.rs 选择是否插入原文）

### 7. multi_model_per_call_timeout

- **Given**: Multi-model 配置 5 个候选 prompt/model
- **When**:
  1. 5 个候选并发启动
  2. 3 个在 2-3 秒成功
  3. 2 个超过 5 秒 → 被 `tokio::time::timeout` 取消
- **Then**:
  - extensions.rs 收集到 3 个成功结果 + 2 个 timeout error
  - 用户拿到 multi-model 结果集（含 3 个候选）
  - 总用时 ≈ 5 秒（被取消的两个不阻塞后续）

### 8. pipeline_timeout_during_multi_model

- **Given**: Multi-model 配置 5 个候选，但全部都卡（无可用结果）；5s 单次超时全部触发
- **When**:
  1. 5 个候选并发跑到 5 秒全部 timeout
  2. extensions.rs 在 ~5 秒返回失败
  3. 10 秒外层 timeout 未触发
- **Then**:
  - 总用时 ≈ 5 秒
  - Pipeline 看到 multi-model 整体失败 → 现有逻辑决定（可能直接结果是空 MultiModel 或转 PassThrough）

### 9. happy_path_race_short_text

- **Given**: Race 配置 A + B；两者都在 1 秒内能返回
- **When**: t=0.8 时 B 先成功
- **Then**:
  - A 的 future 被 drop（自动取消）
  - 总用时 ≈ 0.8 秒
  - 未触发任何 timeout
  - PipelineResult::SingleModel（B 的结果）

## 实施偏差

> 功能完成后填写。记录实际实现与 spec 的差异。

| 原计划 | 实际实现 | 原因 |
| ------ | -------- | ---- |
| —      | —        | —    |
