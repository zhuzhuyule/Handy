---
name: "热词本地匹配与润色提示词质量优化"
tags: [hotword, polish, prompt, routing]
depends_on: []
estimate: "1d"
---

## 意图

"修复热词信息在到达 LLM 之前被丢弃或失效的三个结构性缺陷（别名不注入、纠错对先截断后过滤、needs_hotword 盲判），并以本地确定性匹配替代/兜底 LLM 的热词判断；同时重写轻量润色与智能路由提示词（few-shot + 语言统一），让短文本润色对已知误识形式的纠正率显著提升。"

解决的问题：用户反馈润色实际效果"差点意思"，尤其措辞和热词处理不稳定。根因分析见会话记录：

1. `build_injection_from_ranked` 丢弃 `originals` 别名，LLM 只见正确写法、不知误识形式；
2. 纠错对按星级排序后先 `truncate(15)` 再做输入命中过滤，命中的可能被截掉；
3. `needs_hotword` 由意图模型在看不到热词表的情况下判断，误判 false 时 lite 路径完全跳过热词注入；
4. 路由模型无法识别"通顺但含同音字误识"的文本，PassThrough 误放行；
5. lite/routing 提示词为英文纯规则零示例，跑在小模型上效果差；
6. 轻量润色 prompt 借壳克隆第一个技能，泄漏其 `model_id`（钉了模型的技能会劫持轻量模型选择）；
7. 管线算出的 `detected_language` 未传入 PromptBuilder，输出语言约束基于 UI 语言而非内容语言。

## 约束

- 不改变 `PipelineResult` 枚举与前端契约。
- 不改变 `pipeline_decisions` / `llm_call_log` 表结构；新增的覆盖原因复用现有 `intent_overridden` / `intent_override_reason` 字段。
- 本地热词扫描必须是同步、确定性的 SQLite 读取（无 LLM 调用），单次扫描开销目标 < 5ms。
- 遵守 CLAUDE.md runtime rules（不在非 async 上下文 tokio::spawn 等）。
- 所有 AI 提示词改动保持在 `src-tauri/resources/prompts/*.md` 外部文件中。
- 编译零 warning，现有测试全绿。

## 已定决策

1. **本地热词匹配器放在 `HotwordManager` 上**（`scan_local_match`），返回 `{ alias_hit, target_hit }`。
   - 原因：复用现有连接/scenario 查询逻辑，避免新模块与状态管理。
2. **匹配规则**：复用 `count_hotword_occurrences`（ASCII 词边界 + CJK 子串）；对含空格或 ≥5 字符的 ASCII 词追加"去空格小写"宽松匹配，覆盖 "vo type" ↔ "votype" 类分词变体。
   - 原因：纯增量，不破坏现有词边界保护（"mat" 不命中 "format"）。
3. **PassThrough 否决仅由 alias_hit 触发**（target_hit 表示词已正确，无需润色介入）；否决路径复用现有 repetition 覆盖机制，reason 记 `hotword_alias_match`。
4. **needs_hotword = LLM 判断 OR (alias_hit || target_hit)**：LLM 结果降级为先验。
5. **纠错对选取改为"命中优先"**：先保留与输入文本匹配的 pair（不受截断影响），再按星级补齐至 15。
6. **别名进注入文本**：`HotwordEntry.aliases` 取 `originals` 前 3 个；渲染为 `Target（误识别形式: a / b）`。
7. **轻量润色 prompt 用 `Skill::default()` 干净构造**，`output_mode: Polish`，`model_id: None`。
8. **`detected_language` 传入 PromptBuilder**：新增 `.detected_language()`，输出语言约束优先用它，`app_language` 兜底。full polish 路径（routing.rs/extensions.rs）用启发式语言检测补齐（意图结果不跨函数传递，避免 4 处外部调用方签名变更）。
9. **提示词重写为中文主体 + few-shot**：`system_lite_polish.md`、`system_smart_routing.md`。JSON 字段名保持英文。路由提示词明确"不确定时选 lite_polish""单行 JSON、无 code fence"。
10. **场景检测去重**：`pipeline::detect_scenario` 委托 `hotword::detect_scenario_from_app_name`（提升为 `pub(crate)`）。

## 边界

### 允许修改

- `src-tauri/src/managers/hotword.rs`
- `src-tauri/src/actions/post_process/pipeline.rs`
- `src-tauri/src/actions/post_process/prompt_builder.rs`
- `src-tauri/src/actions/post_process/routing.rs`
- `src-tauri/src/actions/post_process/extensions.rs`（仅 PromptBuilder 调用点加 detected_language）
- `src-tauri/src/actions/post_process/manual.rs`（同上，如改动极小）
- `src-tauri/resources/prompts/system_lite_polish.md`
- `src-tauri/resources/prompts/system_smart_routing.md`

### 禁止

- 不动 `core.rs` 的 LLM 执行层（LlmCallExecutor 统一是独立后续工作）。
- 不动历史缓存查询逻辑与 DB schema。
- 不动 review window / 前端代码。
- 不删除 `maybe_post_process_transcription` 的 legacy 路由层（独立后续工作，影响 4 个外部调用方）。

## 排除范围

- `LlmCallExecutor` 抽取与 extensions.rs HTTP 实现统一。
- 历史缓存键加入 prompt_id/app_category。
- 场景提示（scenario-hint）外置到 resources/prompts。
- rewrite 提示词改 diff-based 输出。
- 多模型策略与候选排序逻辑。

## 验收场景

### 1. alias_hit_vetoes_passthrough（Happy path）

- **Given**: 热词表含 target="Votype"、originals=["窝太普"]，智能路由开启，输入"我在用窝太普写东西"
- **When**: 意图模型返回 pass_through
- **Then**: 管线将动作升级为 LitePolish，`pipeline_decisions.intent_overridden=true`、`intent_override_reason="hotword_alias_match"`，且热词注入包含 `窝太普 → Votype` 纠错对

### 2. correction_pairs_hit_priority（Happy path）

- **Given**: 热词表含 20 个带 originals 的热词，其中恰有 1 个低星级 pair 的 original 出现在输入文本中
- **When**: 构建注入
- **Then**: 该命中 pair 必定在 `correction_pairs` 中（不被 15 条上限截掉）

### 3. aliases_rendered_in_prompt（Happy path）

- **Given**: 热词 target="Votype"、originals=["vo type", "vtype"]
- **When**: PromptBuilder 构建 user message
- **Then**: `[product-names]` 区块包含 "Votype（误识别形式: vo type / vtype）"

### 4. needs_hotword_false_overridden_by_local_hit（Edge case）

- **Given**: 意图模型返回 needs_hotword=false，但输入文本命中热词 alias
- **When**: lite 路径执行
- **Then**: 热词注入照常进行（不被 LLM 误判关闭）

### 5. lite_prompt_no_model_leak（Edge case）

- **Given**: 用户第一个技能钉了 `model_id="some-pinned-model"`，length_routing_short_model 配置为轻量模型
- **When**: LitePolish 路径解析模型
- **Then**: 使用 length_routing_short_model 链，而非被钉的模型

### 6. word_boundary_no_false_positive（Edge case）

- **Given**: 热词 alias="mat"，输入文本 "format the disk"
- **When**: 本地扫描
- **Then**: alias_hit=false（ASCII 词边界保护不回退）

### 7. scan_failure_degrades_gracefully（Error path）

- **Given**: 热词 DB 查询失败（如表缺失）
- **When**: 本地扫描在管线中执行
- **Then**: 扫描结果视为全 false，管线按原有逻辑继续（不 panic、不中断润色）

### 8. detected_language_drives_output_note（Happy path）

- **Given**: UI 语言为 zh，输入为纯英文 "send the report to matt"
- **When**: 构建 prompt
- **Then**: Output Language 约束为英文版本（基于 detected_language="en"，而非 app_language="zh"）

## 实施偏差

| 原计划                                              | 实际实现                                                                          | 原因                                                                                             |
| --------------------------------------------------- | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| 边界内不含 `settings.rs`                            | 额外修复 `ensure_post_process_defaults` 不恢复必需 builtin provider 的存量缺陷    | 该测试在 HEAD 上已失败（非本次引入），同会话顺手修复                                             |
| 未计划改 `has_repetition_pattern`                   | 重写判定规则：叠词（"天天"、"谢谢"）和英文双字母（"hello"）不再误判为口吃         | 该测试在 HEAD 上已失败；且误报导致此类文本永远无法 PassThrough，与本 spec 的路由质量目标直接相关 |
| `detected_language` 仅计划覆盖 lite/full/multi 路径 | 同时覆盖了 legacy lite 路径（routing.rs `execute_smart_polish_lite`）与 manual.rs | 改动极小，保持各路径行为一致                                                                     |
| —                                                   | `format_hotword_entry`（rewrite term_reference 渲染）同步加入别名展示             | 与 prompt_builder 渲染保持一致，rewrite 路径同样受益                                             |
