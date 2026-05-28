---
name: "History 列表行内错误标记"
tags: [dashboard, history, observability, post-processing]
depends_on:
  - "pipeline_decisions 表（已有 error_type / error_detail / bypass_reason 列）"
  - "transcription_history 表（已有 transcription_text / post_processed_text）"
  - "DashboardEntryCard.tsx 现有动作区"
estimate: "1 day"
---

## 意图

"在 Dashboard 历史记录每一项的右侧动作区，给后处理失败的录音加一个橙色 ⚠️ 图标。hover 显示一句话摘要，点击展开行内错误详情面板。同时在 Dashboard 顶部加一个『仅显示失败』开关，让用户怀疑系统有问题时一键过滤。"

解决的问题：当 polish 链路出错（provider 404、超时、所有 fallback 都挂）时，pipeline 会优雅降级到插入 ASR 原文。但目前用户**无从感知**降级发生过——他们看到原文以为"polish 没运行过"，找不到根因。错误数据本来就记在 `pipeline_decisions` 表里，只是没暴露到 UI。

## 约束

- **不改 DB schema**——只读现有 `pipeline_decisions` / `transcription_history` 列。
- **不修改 polish / ASR / LLM 调用路径**——这是纯只读的 UI 增强特性，0 风险动到核心 pipeline。
- V1 只覆盖**两类错误**：Polish 链路失败（含 10s pipeline 超时）+ ASR 空结果。**不**覆盖热词 / 标点的 soft fail（V1.5 议题）。
- 错误信息只在 history item 本地展开/收起，**不弹 modal、不开侧栏、不发 toast**。
- 不引入新的 backend command；扩展现有 `get_history_entries` 的返回类型即可。
- 一条 history 最多对应一条 `error_summary`。一对多关系（多条 `pipeline_decisions` ⇄ 一条 history）时取**按 timestamp 最新**的一条；这样用户重试成功后历史记录不会"永久染色"（验收场景 6）。
- Filter toggle 默认关闭，启用后前端纯客户端过滤（不重新请求后端）。
- specta 类型同步：扩展 `HistoryEntry` 后 bindings.ts 重生成。
- 遵守 CLAUDE.md：i18n 文案放 locales/{en,zh}/translation.json。
- 修改前提交前消除所有 warning。

## 已定决策

- **V1 范围 = Polish + ASR 两类错误。** 不选"全四类"：热词 / 标点的 soft fail 不影响用户能看到/插入文本（只是品质略差），缺乏紧急性；且需新增 DB 字段 + 三处写入路径，代价不成比例。先验证 UI 体验有用再扩展。

- **错误数据来源 = `pipeline_decisions` 表 LEFT JOIN（按 history_id）+ `transcription_history.transcription_text` 空判定。** 不引入第三张表、不动数据写入路径。

- **视觉位置 = 右侧动作区（Insert 按钮左侧）。** 不选左侧文本前小点：会破坏现有文本对齐；动作区位置语义上"这条记录的元信息"更合适。

- **图标颜色 = amber/orange（`color="amber"`）。** 区别于其他动作（蓝色 Insert、灰色 Edit）。明显但不像 red 那样紧迫——降级是问题但不是灾难。

- **交互 = hover 短摘要 tooltip + 点击行内展开。** 不选 modal：行内展开能让用户**并排**看到错误和原文，对比上下文更直观；modal 切走用户上下文。

- **过滤 = 前端客户端过滤。** 不选后端加 `?errors_only=true` 参数：客户端过滤在已加载的 history 上一行 `entries.filter(...)` 就行，复杂度为 0；后端加参数需要 specta + bindings + 加测试。等历史量超过数千条再考虑分页 + 后端过滤。

- **HistoryEntry 扩展 `error_summary: Option<HistoryError>`。** 不在 HistoryEntry 上加 N 个扁平字段（`error_type`、`error_detail`、`error_model`、...）：嵌套结构在前端更清晰，TypeScript 类型也更窄。

- **不加重试按钮。** "重新跑 polish" 是 V2 议题，需要 reprocess 入口。V1 只做可见性。

- **error_summary 来源优先级（确定性单一值）：**
  1. 最近一次 `pipeline_decisions` 行（按 timestamp DESC）的 `error_type` 不为 null → 用它（stage="polish"，error_type 取 pipeline_decisions 列原值，detail 取 error_detail 原值，model 取 selected_model_id）
  2. 上一步未命中 + `transcription_text` 为空字符串 → stage="asr", error_type="asr_empty", detail=null, model=asr_model
  3. 都未命中 → `error_summary=null`

  这个优先级保证 polish 错误**覆盖** ASR 空判定（因为 polish 错误更具诊断价值，且通常 ASR 空时 polish 也会 fail）。

- **stage 字段值固定为小写：** `"polish"` 或 `"asr"`。前端按字符串匹配渲染对应文案。

- **过滤 toggle 位置 = Dashboard 顶部 toolbar 区域。** 不在 entry card 上加：toggle 是 "列表级" 操作，逻辑上属于顶部。

## 边界

### 允许修改

- `src-tauri/src/managers/history.rs`：
  - `HistoryEntry` struct 增加 `pub error_summary: Option<HistoryError>` 字段
  - 新增 `HistoryError` struct（4 个字段：stage / error_type / detail / model）
  - 修改 `get_history_entries` SQL：LEFT JOIN `pipeline_decisions ON pipeline_decisions.history_id = transcription_history.id`
  - SQL 用 GROUP BY + MAX(timestamp) 拿最近一次决策（一条 history 可能对应多条 pipeline_decisions）
- `src/bindings.ts`：specta 重生成
- `src/lib/types.ts`：同步 `HistoryEntry` 的 Zod schema（添加 `error_summary`）
- `src/components/settings/dashboard/DashboardEntryCard.tsx`：
  - 在 Insert 按钮左侧渲染条件性 ⚠️ IconButton（仅当 `entry.error_summary != null`）
  - Tooltip + 点击展开 state
  - 错误面板组件（行内展开）
- Dashboard 顶部 toolbar（具体文件在 plan 阶段确认，初判是 `src/components/settings/dashboard/` 下的列表容器组件）：
  - 加 `errorsOnly` state + Switch
  - `entries.filter(e => !errorsOnly || e.error_summary != null)` 应用到渲染
- `src/i18n/locales/en/translation.json` / `zh/translation.json`：新 key

### 禁止

- 修改 `pipeline_decisions` 表 schema
- 修改 `llm_call_log` 表 schema
- 修改 polish 路径（pipeline.rs / fallback.rs / manual.rs / extensions.rs / routing.rs / core.rs）
- 新增 Tauri command（复用现有 `get_history_entries`）
- 在 entry card 上加重试 / reprocess 按钮
- 弹 modal / toast / 通知中心

## 排除范围

- 热词 / 标点失败的标记（V1.5 议题，需新 DB 字段）
- 错误统计 / 趋势图 / 报表
- 系统级错误（capability 配错、所有 provider 全挂）——仍走 toast / settings warning
- 错误重试 / reprocess 入口
- 错误聚合通知（"过去 1 小时出现 10 次同类错误"）
- 后端过滤（V2 等历史量大时再考虑）
- 排序（按错误优先 / 按时间）—— V2 议题

## 验收场景

### 1. happy_path_no_error_no_icon

- **Given**: 用户录音、polish 成功插入
- **When**: 进 Dashboard 看 history
- **Then**:
  - 对应 history item **不渲染** ⚠️ 图标
  - 动作区只有 Insert / Edit / Copy 等现有按钮
  - `entry.error_summary` 为 `null`

### 2. polish_timeout_shows_icon_and_detail

- **Given**: polish pipeline 触发 10s 超时（spec docs/specs/2026-05-26-polish-pipeline-timeout.spec.md）；`pipeline_decisions` 行 `bypass_reason='timeout'`
- **When**: 用户在 Dashboard 找到这条 history
- **Then**:
  - 右侧动作区出现 amber 色 ⚠️ IconButton
  - hover 显示 Tooltip："Polish 超时"
  - 点击 ⚠️ → 同 row 下方展开错误面板，含：
    - 阶段："Polish"
    - 错误类型："timeout"
    - 详情：完整 error_detail 字符串
    - 时间戳
  - 再点 ⚠️ → 面板收起

### 3. polish_404_provider_error

- **Given**: 主备模型都返 404；`pipeline_decisions.error_type='llm_api_error'`，`error_detail` 含 status=404 信息
- **When**: 进 Dashboard
- **Then**:
  - ⚠️ 图标出现
  - Tooltip："Polish 失败 (404)"
  - 展开面板看到完整错误：`...status=404: Model X does not exist...`，含 model + provider 字段

### 4. asr_empty_shows_icon

- **Given**: ASR 返回空文本，但 history 行因 placeholder 机制仍存在；`transcription_history.transcription_text=''`
- **When**: 进 Dashboard
- **Then**:
  - ⚠️ 图标出现
  - Tooltip："ASR 空结果"
  - 面板显示 stage=`"asr"`, error_type=`"asr_empty"`

### 5. filter_toggle_shows_only_errors

- **Given**: 用户有 20 条 history，其中 3 条有错误
- **When**: 点击 Dashboard 顶部"仅显示失败"开关 → 启用
- **Then**:
  - 列表只显示 3 条带 ⚠️ 的 history
  - 关闭开关 → 恢复显示 20 条
  - URL / query params 不变（纯客户端 state）

### 6. multiple_pipeline_decisions_pick_latest

- **Given**: 同一条 history 因为某种原因（例如用户在 review 窗口重试） 有 2 条 `pipeline_decisions` 行：第一条 error_type='llm_timeout'，第二条 error_type=null（成功）
- **When**: 进 Dashboard
- **Then**:
  - 用最新的一条决策（成功）
  - `entry.error_summary` 为 `null`
  - **不**渲染 ⚠️
  - 即：用户后续重试成功的话，错误就不再展示（避免历史记录长期"染色"）

### 7. typescript_types_synced

- **Given**: 后端扩展 `HistoryEntry` 加 `error_summary` 字段
- **When**: 运行 `bun tauri dev` 触发 specta 重生成；运行 `bun run build`
- **Then**:
  - `src/bindings.ts` 中 `HistoryEntry` 类型含 `error_summary: HistoryError | null`
  - `HistoryError` 类型定义包含 stage / error_type / detail / model
  - `bun run build` 无 TS 错误
  - `src/lib/types.ts` 的 Zod schema 同步更新

### 8. edge_case_pipeline_decisions_no_history_id

- **Given**: `pipeline_decisions` 表中有些行的 `history_id` 是 null（旧数据或边缘场景）
- **When**: 查询 history
- **Then**:
  - 这些孤立的 pipeline_decisions 行被忽略
  - 不影响 history 列表渲染
  - 不导致 SQL JOIN 报错

## 实施偏差

> 功能完成后填写。记录实际实现与 spec 的差异。

| 原计划 | 实际实现 | 原因 |
| ------ | -------- | ---- |
| —      | —        | —    |
