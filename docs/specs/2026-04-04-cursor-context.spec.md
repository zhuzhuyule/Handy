---
name: "光标上下文注入"
tags: [post-processing, context, accessibility, macos]
depends_on: []
estimate: "3h"
---

## 意图

"通过 macOS Accessibility API 获取光标所在位置的前后文本，作为弱参考上下文注入到后处理 prompt 中，帮助 LLM 消歧义和理解用户意图，从而提高 ASR 后处理的准确率。"

当前问题：

1. 后处理 pipeline 缺少用户正在编辑内容的上下文，LLM 只能看到当前这句语音转写的文本
2. 相同的语音内容在不同编辑上下文中含义可能完全不同（如"他"指谁、"这个"指什么）
3. 现有的 `session_context`（最近 5 条同 app 转写）是语音维度的上下文，缺少文档维度的上下文

## 约束

- macOS Accessibility API 需要用户已授权辅助功能权限（Votype 已有此权限，因为 `get_selected_text_via_accessibility()` 依赖它）
- 不是所有 app 都支持 `AXValue` / `AXSelectedTextRange`，获取失败必须静默跳过
- AXValue 可能返回超大文本（万字文档），必须有长度保护：AXValue 超过 100,000 字符时放弃获取
- 光标上下文是弱参考，不能让 LLM 把上下文内容混入输出
- 不持久化光标上下文内容（隐私）

## 已定决策

1. **纯 Accessibility API 方案**：只用 `AXValue` + `AXSelectedTextRange` 获取光标周围文本，不模拟按键、不碰剪贴板、不监听键盘输入。原因：零额外权限成本，无用户感知，实现最简单。

2. **前重后轻截取**：光标前最多 300 字符，光标后最多 100 字符。原因：用户通常在续写，前文比后文更有价值。

3. **按句/段边界截断**：在截取范围内找最近的段落分隔符（`\n`）或句终标点（`。！？.!?`），如果没有则退而在空格/CJK 字符边界截断，极端情况下硬截断。原因：避免截断半句话，保持语义完整。

4. **与 selected_text 互斥**：有选中文本时不获取光标上下文。原因：selected_text 是用户主动选择的明确信号，cursor_context 是被动推断的弱信号，同时存在时 selected_text 优先级更高，额外的 cursor_context 可能冗余或干扰。

5. **仅在 LitePolish 和 FullPolish 注入**：不在 Intent 分析阶段注入（保持轻量），不在 PassThrough / History 缓存命中 / ReviewRewrite 时注入。原因：LitePolish 和 FullPolish 是实际调用 LLM 做润色的路径，上下文消歧价值最大；Intent 分析追求快速决策，额外 token 开销不值得。

6. **失败静默**：获取失败返回 `Err`，调用方 `.ok()` 忽略，不影响主流程。原因：光标上下文是锦上添花，不是必需品。

7. **仅 macOS**：非 macOS 平台提供空实现返回 `Err`。Windows UI Automation 实现作为后续工作。

## 边界

### 允许修改

- `src-tauri/src/clipboard.rs` — 新增 `CursorContext` 结构体和 `get_cursor_context()` / `get_cursor_context_via_accessibility()` 函数
- `src-tauri/src/actions/transcribe.rs` — 采集 cursor_context，传入 pipeline
- `src-tauri/src/actions/post_process/pipeline.rs` — `unified_post_process()` 签名新增 `cursor_context` 参数，传入 PromptBuilder
- `src-tauri/src/actions/post_process/prompt_builder.rs` — 新增 `CursorContext` 字段、`FieldTag::CursorContext`、渲染逻辑、Input Protocol 规则
- `src-tauri/src/actions/post_process/extensions.rs` — 如需透传 cursor_context 到 multi-model 执行

### 禁止

- 不修改 prompt 模板文件（`src-tauri/resources/prompts/*.md`）— 上下文通过 PromptBuilder 动态注入
- 不持久化 cursor_context 原文到数据库或日志文件
- 不在 Intent 分析路径中注入 cursor_context
- 不为获取失败提供剪贴板模拟回退

## 排除范围

- Windows / Linux 平台支持（后续通过 UI Automation API 实现）
- 剪贴板内容作为上下文（隐私风险高）
- 光标上下文的历史缓存或定时快照
- 新的用户设置项（光标上下文默认启用，随 smart routing 开关控制）
- `pipeline_decisions` 表 schema 变更（使用现有的扩展字段记录 `has_cursor_context`）

## 验收场景

### 1. happy_path_native_app

- **Given**: 用户在 macOS 原生 TextEdit 中光标前有 "关于明天的会议，我想确认"，光标后有 "请回复"
- **When**: 用户语音输入 "参会人员名单"，pipeline 进入 LitePolish/FullPolish
- **Then**: PromptBuilder 生成的 user message 包含 `[cursor-context]` section，before 包含 "关于明天的会议，我想确认"，after 包含 "请回复"

### 2. happy_path_with_selected_text

- **Given**: 用户在编辑器中选中了一段文本
- **When**: 用户语音输入触发后处理
- **Then**: `cursor_context` 为 None，仅注入 `[selected-text]`，不注入 `[cursor-context]`

### 3. happy_path_cursor_at_end

- **Given**: 用户光标在文档末尾，前方有文本，后方为空
- **When**: 语音输入触发后处理
- **Then**: `[cursor-context]` 只包含 `--- before cursor ---` 部分，不包含 `--- after cursor ---`

### 4. boundary_truncation

- **Given**: 光标前 500 字符内有句号，光标后 200 字符内有换行
- **When**: 获取 cursor context
- **Then**: before 在句号后截断（不超过 300 字符），after 在换行前截断（不超过 100 字符）

### 5. error_unsupported_app

- **Given**: 用户在不支持 AXValue 的 app 中（如某些 Electron app）
- **When**: `get_cursor_context_via_accessibility()` 调用失败
- **Then**: 返回 `Err`，调用方 `.ok()` 得到 None，pipeline 正常执行，无 `[cursor-context]` section

### 6. error_huge_document

- **Given**: 用户在一个超过 100,000 字符的文档中编辑
- **When**: `AXValue` 返回超长文本
- **Then**: 函数检测到长度超限，返回 `Err`，不尝试截取

### 7. edge_no_boundary_found

- **Given**: 光标前 300 字符内没有任何段落/句子边界符（如一个超长无断句的 URL 或代码行）
- **When**: 获取 cursor context
- **Then**: 退而在空格或字符边界截断；如果连空格都没有，硬截断在 300 字符处

### 8. edge_passthrough_no_injection

- **Given**: Intent 分析判定为 PassThrough
- **When**: pipeline 执行 PassThrough 路径
- **Then**: 不注入 cursor_context（PassThrough 不经过 LLM）

### 9. edge_review_rewrite_no_injection

- **Given**: 用户在 review 窗口发起 voice rewrite
- **When**: pipeline 以 ReviewRewrite 模式执行
- **Then**: 不获取 cursor_context（review 窗口已有 frozen editor content 作为上下文）

## 实施偏差

> 功能完成后填写。记录实际实现与 spec 的差异。

| 原计划                                                                                 | 实际实现                                                                        | 原因                                                                |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| 使用现有扩展字段记录 has_cursor_context                                                | 新增 migration 41: ALTER TABLE pipeline_decisions ADD COLUMN has_cursor_context | PipelineDecisionRecord 无 extras 字段，需新增列                     |
| extensions.rs 如需透传 cursor_context                                                  | 未修改 extensions.rs，multi-model 路径无 cursor_context                         | extensions.rs 有独立 HTTP 实现，统一为后续工作（与 CLAUDE.md 一致） |
| test_truncate_before_at_sentence_boundary 期望 "Second sentence. Third sentence here." | 实际断言 "Third sentence here."                                                 | 计划中的期望值计算有误，实际算法在 30% 搜索窗口找到 `.` 后跳过空格  |
