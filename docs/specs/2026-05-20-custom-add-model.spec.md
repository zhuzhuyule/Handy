---
name: "添加模型对话框：自定义输入并测试后入库"
tags: [settings, post-processing, models, llm]
depends_on: []
estimate: "1 day"
---

## 意图

"在 `AddModelDialog` 的搜索栏右侧加一个『+ 自定义』按钮，点击弹出叠加的子对话框。子对话框允许用户手动填模型 ID、使用类型、显示名、Body 参数、Headers，先调一次 ping 推理验证连通性，看到响应内容/速度/thinking 标记后再入库。测试默认必需，但允许『跳过测试直接添加』。"

解决的问题：部分 provider 不返回 `/models` 列表或返回不完整，导致用户无法通过现有 Free / API 两个 tab 添加这类模型；用户也希望在添加前以最终生效的 headers / body 参数做一次真实推理验证，而不是先盲存再去 EditModelDialog 调参。

## 约束

- 复用现有后端命令 `test_post_process_model_inference`，扩展其签名而不是新增并行命令（避免出现两条几乎相同的测试路径）。
- 现有 Free / API tab 的逻辑不动，新增能力作为第三种入口而不是替换。
- 子对话框必须叠加在 `AddModelDialog` 之上，关闭后回到主对话框且其状态保留（搜索框文本、SegmentedControl 选中项、已选模型）。
- 测试调用必须把用户当前在子对话框里填的 `extra_params` 和 `extra_headers` 应用到请求上——否则"测试"和"添加后实际使用"行为不一致，违背先测后存的初衷。
- thinking 注入策略：自定义路径下，用户通过『启用思考 / 禁用思考』预设按钮把 thinking 字段写进 `extra_params`，不再走旧路径里依赖 CachedModel `is_thinking_model` 的自动注入。
- 必须复用 `KeyValueEditor`（与 `EditModelDialog` 同款），保证 Body 参数 / Headers 的编辑体验一致。
- 遵守 CLAUDE.md：所有 AI prompt 走 PromptManager 外置文件（本特性使用的『一句话 ping』复用 `test_post_process_model_inference` 内置的 `你是啥模型？` prompt，不引入新 prompt 文件）。
- 修改前提交前消除所有 warning。
- 前端类型必须通过 specta 重新生成 `src/bindings.ts`。

## 已定决策

- **入口位置：搜索框旁边的常驻『+ 自定义』按钮。** 不选"无搜索结果时内联出现"（发现性弱、新用户找不到），也不选"第三个 tab"（语义上和 Free / API 不平级——自定义是兜底入口，不是模型来源）。按钮始终可见，鼠标 hover 提示 "添加自定义模型 ID"。

- **测试强制性：默认必需通过，可跳过。** 「添加」按钮默认 disabled；测试通过 _或_ 用户点了『跳过测试直接添加』链接后启用。
  - 「跳过测试直接添加」link 可见性：ID 非空 _且_ (从未测试 _或_ 测试失败) 时可见；ID 为空 _或_ 测试通过时隐藏。
  - 不选"必须通过才能添加"：provider 可能临时 5xx、用户可能要预添加待会儿用的模型。
  - 不选"完全可选"：默认不测试会让用户漏检低级错误（拼错 ID、headers 写错）。

- **测试语义：一句话固定 prompt（`你是啥模型？`）→ 非错误响应即通过。** 展示响应正文（截断 200 字 + 展开）、token 速度（t/s）、是否含 thinking。
  - 不选"双层（先 /models 再推理）"：列表 endpoint 不可靠（很多 provider 返回不全或不返回），把推理作为唯一信号更直接。
  - 不选"用户自定义 prompt"：增加面板复杂度且非核心场景。

- **表单字段：ID + 使用类型 + 显示名 + Body 参数 + Headers。** 不放 model_family（可由 EditModelDialog 后续设置，多数 provider 不需要）。
  - 不选"最小化只填 ID"：自部署 / 代理网关 provider 几乎总是需要额外 headers 才能调通；最小化会逼用户走"加完再编辑"的迂回路径。
  - 不选"全量包含 model_family + thinking 自动注入"：thinking 注入逻辑依赖 CachedModel 持久态，自定义路径下用户已能通过『启用思考』按钮显式写入，不再做"猜测"。

- **面板呈现：叠加子对话框。** 不选"原地替换网格"：主对话框已有的状态（已选模型、搜索关键字、source tab）切换后保留更直观；子对话框在 z-index 上盖住主对话框，关闭后两者状态独立。

- **后端命令：扩展 `test_post_process_model_inference` 增加两个可选 override 参数。** 不新增并行命令（如 `test_inline_inference`）：旧调用路径 `cached_model_id=Some(...)` 与新路径 `extra_params_override=Some(...), extra_headers_override=Some(...)` 在同一个命令内分支处理，签名变化对现有调用者保持向后兼容（新参数默认 None）。
  - 当 `extra_params_override` 或 `extra_headers_override` 为 `Some` 时：跳过 `cached_models` 查找，直接用 override；thinking auto-inject 路径不参与（用户必须显式写）。
  - 当两者均为 `None` 且 `cached_model_id=Some`：保持现有行为。

- **重复 ID 策略：警告但不阻断。** 若 `cached_models` 中已存在同 `(provider_id, model_id)`，在子对话框顶部展示 inline warning："已存在同名模型，仍可添加（会成为副本）"。用户可能想用不同 headers / body 参数维护多个变体。

- **错误信息分类：原样展示后端字符串，不做前缀映射。** `test_post_process_model_inference` 返回 `Result<InferenceResult, String>`，错误字符串就是后端 detail（含 status code）。前端在测试失败面板直接展示——不像 `quick_insert_to_target` 需要按 toast 文案分类，这里用户直接读完整错误更有诊断价值。

- **InferenceResult 信号映射：**
  - 「响应内容」← `InferenceResult.content`（去掉前后空白；空字符串视为失败）
  - 「速度」← `total_tokens` / `duration_ms * 1000`，仅在两者非 None 且 ms > 0 时展示
  - 「Thinking 标记」← `reasoning_content` 非空 _或_ `is_thinking` flag（参考 `usePostProcessProviderState.ts` 的 testInference 中 `hasThinking` 计算）

- **图标：** 主对话框入口按钮用 `IconPlus`（项目已用，见 EditModelDialog line 12）；子对话框无独立 icon。

- **成功添加后流程：** 添加完成后子对话框**自动关闭** → 主对话框保持打开 → `cachedModels` 列表自动刷新（`addCachedModel` 已触发 settings 同步）。不需要额外提示 toast——用户能在主对话框其他 tab 切回后看到该 ID 出现 `已添加` 徽章。

## 边界

### 允许修改

- 新建：
  - `src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx`
- 修改：
  - `src/components/settings/post-processing/dialogs/AddModelDialog.tsx`：搜索栏右侧加 `+ 自定义` 按钮 + 触发子对话框 state
  - `src-tauri/src/shortcut/test_cmds.rs`：扩展 `test_post_process_model_inference` 签名（新增 2 个 `Option<HashMap<String, serde_json::Value>>` 参数）
  - `src/bindings.ts`：specta 重新生成
  - `src/stores/settingsStore.ts`：`testPostProcessInference` 函数签名增加可选 overrides 参数，invoke 时透传
  - `src/hooks/useSettings.ts`：暴露的 `testPostProcessInference` 类型同步
  - `src/components/settings/PostProcessingSettingsApi/usePostProcessProviderState.ts`：`testInference` 不需要改动（保持现有"按 modelId 测试已存在模型"语义），但允许新增一个 `testInferenceInline(modelId, extraParams, extraHeaders) -> Promise<...>` helper 供 CustomAddModelDialog 使用

### 禁止

- 修改现有 `Free / API` SegmentedControl 切换逻辑、modelOptions 构建逻辑（`AddModelDialog.tsx` line 159-184）
- 修改 `EditModelDialog.tsx`（添加后编辑用现有面板，不复制其代码）
- 修改 `add_cached_model` / `CachedModel` 数据结构
- 修改 `KeyValueEditor` 组件本身
- 引入新的 AI prompt 文件
- 在主对话框上额外引入"自定义"作为 SegmentedControl 的第三个 tab

## 排除范围

- 批量自定义添加（一次填多个 ID）。
- 自定义 prompt 测试（让用户自填测试问题）。
- 测试历史 / 测试失败原因聚合分析。
- model_family 字段在子对话框中可填。
- 自动从用户填的 ID 推断 model_type（比如 ID 含 `whisper` 自动设 ASR）——一律默认 Text，用户自选。
- 测试通过后自动给 `is_thinking_model` 打标（thinking 由用户预设按钮显式管理）。
- 修改 `usePostProcessProviderState.ts` 中 `testConnection` / `verifiedProviderIds` 相关逻辑。
- provider 不存在时的兜底添加（这是 provider 配置层的问题，不在本 spec 范围）。

## 验收场景

### 1. happy_path_test_then_add

- **Given**: 用户在 PostProcessingPanel 选中某个自部署 OpenAI 兼容 provider，打开 `AddModelDialog`
- **When**:
  1. 点击搜索栏右侧 `+ 自定义` 按钮
  2. 子对话框打开，填入 `model_id = "my-custom-llama"`
  3. 在 Headers 中加 `X-Custom-Auth: secret`
  4. 点「测试」
  5. 后端返回 `InferenceResult { content: "我是 LLaMA 微调版", total_tokens: Some(12), duration_ms: Some(800), reasoning_content: None }`
  6. 点「添加」
- **Then**:
  - 测试结果面板显示绿色 ✓，响应内容 "我是 LLaMA 微调版"、速度 "15.0 t/s"，无 thinking 标记
  - 「添加」按钮在测试通过后启用
  - `addCachedModel` 被调用一次，`extra_headers = { "X-Custom-Auth": "secret" }` 被持久化
  - 子对话框关闭，主对话框保持打开
  - `cached_models` 数组新增一项，model_id = "my-custom-llama"，model_type = "text"

### 2. error_path_test_failure_still_allows_skip_add

- **Given**: 子对话框已填 `model_id = "non-existent"`，无额外 headers
- **When**:
  1. 点「测试」
  2. 后端返回 `Err("API request failed with status 404: Model not found")`
  3. 用户点「跳过测试直接添加」link
  4. 点「添加」
- **Then**:
  - 测试结果面板显示红色 ✗，错误正文 "API request failed with status 404: Model not found"
  - 「跳过测试直接添加」link 在测试失败后可见
  - 点击 link 后「添加」按钮启用
  - 模型仍被加入 `cached_models`（用户自担风险）

### 3. edge_case_duplicate_id_warning_but_allow

- **Given**: provider 下已存在 `model_id = "gpt-4"` 的 CachedModel
- **When**: 用户在子对话框输入 `model_id = "gpt-4"`
- **Then**:
  - 子对话框顶部出现 inline warning："已存在同名模型，仍可添加（会成为副本）"
  - 测试 + 添加流程不被阻断
  - 添加成功后 `cached_models` 中存在两条 `model_id="gpt-4"` 但 `id` 不同的记录

### 4. edge_case_empty_id_disables_test_button

- **Given**: 子对话框刚打开，模型 ID 字段为空
- **When**: 用户尚未输入或清空了 ID
- **Then**:
  - 「测试」按钮 disabled，hover tooltip 显示 "请先填入模型 ID"
  - 「添加」按钮 disabled
  - 「跳过测试直接添加」link 不可见（无 ID 时也不允许跳过）

### 5. edge_case_thinking_detected_in_response

- **Given**: 用户填 `model_id = "qwen3-thinking"`，在 Body 参数中通过『启用思考』按钮写入 `{ "thinking": { "type": "enabled" } }`
- **When**:
  1. 点「测试」
  2. 后端返回 `InferenceResult { content: "OK", reasoning_content: Some("用户在问我是什么模型..."), ... }`
- **Then**:
  - 测试结果面板除响应内容外，额外展示 🧠 Thinking 标记
  - 用户能在视觉上确认 thinking 字段生效

### 6. happy_path_close_subdialog_preserves_main_state

- **Given**: 主对话框中用户已切到 API tab、搜索框填了 "gpt"、勾选了 2 个模型
- **When**:
  1. 点 `+ 自定义`，子对话框打开
  2. 子对话框中填写 ID 后点「取消」关闭子对话框
- **Then**:
  - 主对话框 SegmentedControl 仍指向 API
  - 搜索框仍是 "gpt"
  - 已勾选的 2 个模型仍保持勾选
  - 子对话框关闭，不留任何 toast / 焦点丢失

### 7. error_path_backend_signature_backward_compat

- **Given**: 现有调用方 `ModelConfigurationPanel.tsx` line 142 用旧签名调 `test_post_process_model_inference({ modelId, providerId, cachedModelId })`
- **When**: 本次扩展后端命令签名（新增两个可选 override 参数）
- **Then**:
  - 旧调用不传 override 参数，命令仍按 `cached_model_id` 查找 CachedModel 走原路径
  - 单元测试覆盖：`override=None, cached_model_id=Some` 行为不变（thinking 注入仍生效）
  - 单元测试覆盖：`override=Some, cached_model_id=None` 时使用 override，跳过 cached_models 查找
  - specta 重新生成的 `src/bindings.ts` 类型签名包含两个新可选字段

### 8. edge_case_test_with_no_extra_config

- **Given**: 用户填 ID + Display Name，Body 参数和 Headers 均为空
- **When**: 点「测试」
- **Then**:
  - 后端 `extra_params_override = None`、`extra_headers_override = None`
  - 但 `cached_model_id = None`（自定义路径），故不查 CachedModel
  - 请求按 provider 默认配置发出（仅 API key）
  - 测试通过后可正常添加

## 实施偏差

> 功能完成后填写。记录实际实现与 spec 的差异。

| 原计划 | 实际实现 | 原因 |
| ------ | -------- | ---- |
| —      | —        | —    |
