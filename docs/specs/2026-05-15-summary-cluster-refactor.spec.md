---
name: "Summary 页面 cluster 为轴重构"
tags: [summary, task-cluster, llm, ai-feedback-loop, refactor]
depends_on: []
estimate: "3-4 周"
---

## 意图

把 Summary 页面从「Recap + Stats + 启发式 Task Cluster 平铺」重构成「**LLM 语义聚类的 Task Cluster 为页面主轴，辅助信息折叠至侧抽屉**」，并打通 **cluster → 用户编辑 → 反馈 → 下次 prompt** 的闭环。

核心意图："让用户每天看到的不是『AI 给我一堆只读卡片』，而是『AI 提出聚类建议、我能编辑/反馈、下一次生成会变得更准』"。

驱动力：当前 `build_recap()` 是 Rust 启发式规则（按 20 分钟时间间隔 + 8 分钟应用切换分段），不是 AI；cluster 无源链接、无可编辑、无反馈机制；用户对 AI 总结产物没有改进路径。

## 约束

- 仅重构 Summary 页面（Sidebar 第 2 项 / Ctrl+2）。Dashboard 页面（Ctrl+1）本次不动。
- AI 聚类**仅在日内进行**，跨天不合并。week/month 视图是「按日 cluster 卡片的网格聚合」，不调用 LLM 做跨天归纳。
- 所有 AI prompt 必须放在 `src-tauri/resources/prompts/*.md`，由 `PromptManager` 加载，禁止硬编码在 Rust。
- 必须复用现有 LLM 执行层（`execute_llm_request_with_retry`），不要为 cluster 重新写 HTTP。
- 必须遵守 `CLAUDE.md` 中的运行时规则（不在 coordinator 线程用 `block_on`、不在非 async 上下文用 `tokio::spawn`）。
- TaskCluster 必须能被独立寻址（用于反馈附着、用户编辑保护、源转录链接），不能继续以 JSON 数组嵌在 `summaries.stats` 中。
- LLM 调用必须可缓存：同日重复打开不触发 LLM；只有用户主动点 ⟳重新生成 或 `force=true` 才重新调用。
- 失败降级：LLM 网络错 / JSON 解析错时必须保留旧 cluster 数据，不能清空用户已编辑的内容。

## 已定决策

### 信息架构

- **Day View 为主视图**：cluster 列表占据主体；时间选择控件与⟳重新生成按钮放在顶部窄条；Stats / Recap / Profile / Hotword / Export / Feedback History 折叠到 AUX 抽屉。
  - 原因：用户重构动机是「cluster 为轴」，辅助内容必须在视觉权重上让位，但不能丢功能。
- **Week / Month View 不跨天合并 cluster**，呈现为「按日的 cluster 卡片网格」+ 顶部周/月关键词云。
  - 原因：与「仅日内聚类」边界一致；跨日「同一主题」靠关键词重合与用户手动 tag 高亮，而非 AI 跨天再聚类。
- **AUX 抽屉默认折叠**，单行 chips 形式入口，点击展开右侧抽屉。
  - 原因：保留对老功能的访问，但默认视线聚焦 cluster。

### 数据模型

- 新增表 `task_clusters`（独立实体，含 UUID 主键、`source_history_ids_json`、`is_user_modified` 标志位、`user_modified_fields` JSON 数组）。
  - 原因：cluster 必须能被反馈与编辑独立寻址；JSON 嵌入 stats 无法支持高频更新和跨日查询。
- 新增表 `cluster_feedback`（cluster_id FK、thumb、note、created_at）。
  - 原因：反馈是时间序列数据，需支持多条记录、独立删除、跨 cluster 聚合查询。
- `summaries.stats` 移除 `task_clusters` 字段；migration 时把旧 JSON 数据迁移到新表，`is_user_modified=0` 且 `source_history_ids_json='[]'`（历史数据无源链接但仍可见）。
- 当用户编辑 cluster 任意字段（title / status / next_step）→ `is_user_modified=1`，再生时不被 AI 覆盖。
- 当用户拆分（split）或合并（merge）cluster → 双方都标 `is_user_modified=1`。

### LLM 聚类设计

- 新 prompt 文件 `src-tauri/resources/prompts/system_task_clustering.md`，遵循 `{{variable}}` 模板规范。
- 输入数据按规模降级：≤50 条用 `transcription_text`，51-150 条用 `post_processed_text`，>150 条截断单条到 200 字符并丢弃 <5 字符噪音。
- prompt 注入两类上下文：
  - **protected_clusters**：当日 `is_user_modified=1` 的 cluster 的标题与 source_ids（让 AI 不要再分组它们的 entries）。
  - **user_feedback**：最近 30 天 `thumb='down' AND note IS NOT NULL` 的反馈条目，最多 5 条。
- 仅注入 👎+note，不注入 👍。
  - 原因：👎+note 是明确的纠错信号；👍 易过拟合用户偏好，且无具体改进方向。
- 缓存策略：同日 cluster 存在且 `created_at < 1h` 则跳过 LLM；用户点 ⟳重新生成强制 `force=true`。
- 失败降级：网络/解析错时保留旧数据 + 错误 toast；JSON 解析失败立刻严格模式重试一次。

### 用户操作（6 个原子操作）

| 操作                                          | 后端 command                                                                |
| --------------------------------------------- | --------------------------------------------------------------------------- |
| Rename title / Change status / Edit next_step | `update_task_cluster_field(cluster_id, field, value)`                       |
| Split                                         | `split_task_cluster(cluster_id, source_ids_to_extract, new_title)`          |
| Merge                                         | `merge_task_clusters(target_id, source_cluster_ids)`                        |
| Delete                                        | `delete_task_cluster(cluster_id)`（硬删，下次 regenerate 可被 AI 重新提出） |
| Thumb ± 备注                                  | `add_cluster_feedback(cluster_id, thumb, note?)`                            |
| 跳源转录                                      | 前端导航到 Dashboard 中对应 entry_id（无需新 command）                      |

### 代码结构

- 新建目录 `src/components/settings/summary/cluster/`，含 6 个组件（ClusterCard、ClusterDetailDrawer、ClusterFeedbackButtons、SplitClusterDialog、MergeClusterDialog、DeleteClusterConfirm）。
- 新建目录 `src/components/settings/summary/views/`，含 DayView / WeekView / MonthView。
- 新建目录 `src/components/settings/summary/aux/`，把旧 AiAnalysisSection.tsx（755 行）拆成 RecapSection / ProfileSection / HotwordSection / ExportSection / StatsSection / FeedbackHistorySection。
- 新增 Zustand store `summaryStore.ts`：仅存 UI state（viewMode / selectedDate / expandedClusterIds / auxPanelOpen / auxActiveSection），数据本身走 hook 缓存。
- 新增 hooks：`useTaskClusters` / `useClusterFeedback`，保留并瘦身 `useSummary`。
- 后端新增 `managers/task_clusters.rs` + `managers/cluster_feedback.rs` + `commands/task_clusters.rs` + `commands/cluster_feedback.rs` + `actions/task_cluster_generator.rs`。
- 后端删除 `summary.rs` 中的 `build_recap()` 启发式（~200 行）。

## 边界

### 允许修改

**前端：**

- `src/components/settings/summary/SummaryPage.tsx`（瘦身到 ~300 行）
- `src/components/settings/summary/SummaryStats.tsx`（移至 `aux/StatsSection.tsx`）
- `src/components/settings/summary/SummaryCalendar.tsx`
- `src/components/settings/summary/AiAnalysisSection.tsx`（删除，拆分到 `aux/`）
- `src/components/settings/summary/SummaryTimeline.tsx`（移至 `aux/RecapSection.tsx` 内）
- `src/components/settings/summary/summaryTypes.ts`（扩展 TaskCluster 增加 `id` / `source_history_ids` / `is_user_modified` 字段；新增 `ClusterFeedback` 类型）
- `src/components/settings/summary/hooks/useSummary.ts`（瘦身）
- 新建 `src/components/settings/summary/cluster/**`
- 新建 `src/components/settings/summary/views/**`
- 新建 `src/components/settings/summary/aux/**`
- 新建 `src/components/settings/summary/shared/{PeriodSelector,RegenerateButton}.tsx`
- 新建 `src/components/settings/summary/hooks/{useTaskClusters,useClusterFeedback}.ts`
- 新建 `src/components/settings/summary/stores/summaryStore.ts`

**后端：**

- `src-tauri/src/managers/summary.rs`（删除 `build_recap()` 与启发式辅助函数）
- 新建 `src-tauri/src/managers/task_clusters.rs`
- 新建 `src-tauri/src/managers/cluster_feedback.rs`
- 新建 `src-tauri/src/commands/task_clusters.rs`
- 新建 `src-tauri/src/commands/cluster_feedback.rs`
- 新建 `src-tauri/src/actions/task_cluster_generator.rs`
- 新建 `src-tauri/resources/prompts/system_task_clustering.md`
- migrations：在相应 manager 中追加 `M::up("CREATE TABLE IF NOT EXISTS task_clusters ...")` 与 `M::up("CREATE TABLE IF NOT EXISTS cluster_feedback ...")` 条目（与项目现有 `history.rs` 中 migration 风格一致，使用 `rusqlite_migration` 的 `M::up()` Rust 代码，非独立 SQL 文件）
- `src-tauri/src/lib.rs`（注册新 commands）

### 禁止

- **不动 Dashboard 页面**任何文件（`src/components/settings/dashboard/**`）。
- **不动 transcription_history 表结构**，cluster 通过 `source_history_ids_json` 单向引用。
- **不修改全局设置 store**（`src/stores/settingsStore.ts`），新建 `summaryStore` 仅服务于 Summary 页面 UI state。
- **不引入新的 LLM HTTP 客户端**，必须复用 `execute_llm_request_with_retry`。
- **不动 PromptManager 的加载逻辑**，仅新增 prompt 文件。

## 排除范围

- 跨天 AI 聚类（"project / theme" 概念）：保留为未来方案 C，本次不做。
- 实时增量聚类（每条新转录实时归属 cluster）：本次不做。
- Cluster 与外部任务系统（Linear / Jira / Notion）联动：本次不做。
- Recap、UserProfile、Hotword 的算法升级：仅做组件拆分与位置降级，逻辑不变。
- E2E 自动测试（Playwright/Tauri WebDriver）：本次不引入测试框架，依赖手动验证 + 单元测试。
- 反馈匿名化 / 用户级别 prompt 优化：feedback note 直接注入 prompt，不做语义压缩。
- 性能优化超出 spec 中记录的基准值：仅记录，不强制阻断发布。

## 验收场景

### 1. 首次生成（happy path）

- **Given**：2026-05-15 当日有 30 条 transcription_history 记录，task_clusters 表中无该日数据
- **When**：用户首次打开 Day View（selectedDate=2026-05-15）
- **Then**：前端自动触发 `generate_task_clusters('2026-05-15', force=false)`，后端调用 LLM，返回 3-8 个 cluster，每个含完整字段（title/status/source_history_ids 等），写入 `task_clusters` 表，UI 渲染卡片列表按 total_duration_ms 降序

### 2. 缓存命中（happy path）

- **Given**：当日已有 cluster，`created_at` 距今 < 1h，全部 `is_user_modified=0`
- **When**：用户再次打开 Day View
- **Then**：后端不调用 LLM（pipeline_decisions 中无新增 cluster_generation 记录），直接返回缓存结果

### 3. 重命名后保护（happy path）

- **Given**：用户把 cluster A 的 title 从 "OAuth 工作" 改为 "OAuth 调试"，A 的 `is_user_modified=1`
- **When**：用户点击 ⟳重新生成（force=true）
- **Then**：A 仍叫 "OAuth 调试"；A 的 source_history_ids 不出现在任何新 AI 生成的 cluster 中；其他原 `is_user_modified=0` 的 cluster 被删除并重新生成

### 4. 反馈注入下次 prompt（happy path）

- **Given**：用户给 2026-05-14 的 cluster B 添加 👎 + note "Slack 消息应该单独成簇"
- **When**：2026-05-15 用户首次触发 generate
- **Then**：发送给 LLM 的 prompt 内 USER 部分包含该 note 文本；LLM 输出的 cluster 中 Slack 类应用倾向单独成簇（非强校验，记录用于回归观察）

### 5. 拆分（happy path）

- **Given**：cluster A 含 source_history_ids=[10,11,12,13,14,15,16,17]，total_duration_ms=3600000
- **When**：用户在 SplitClusterDialog 选中 [11,13,15]，输入新标题 "X"，点击拆分
- **Then**：原 A 的 source_history_ids 变为 [10,12,14,16,17]，重算 entry_count=5、total_duration_ms；新建 cluster X 含 source_history_ids=[11,13,15]、is_user_modified=1；A 也 `is_user_modified=1`

### 6. 合并（happy path）

- **Given**：cluster A 含 source_ids=[10,11,12]，cluster B 含 source_ids=[20,21]
- **When**：用户在 cluster A 的 MergeClusterDialog 选中 B 作为合并源
- **Then**：A.source_history_ids=[10,11,12,20,21]，entry_count=5，total_duration_ms 重算；B 被 DELETE；A 的 title 保持不变；A `is_user_modified=1`

### 7. 网络错误降级（error path）

- **Given**：当日已有缓存 cluster，用户网络断开
- **When**：用户点击 ⟳重新生成
- **Then**：后端 `execute_llm_request_with_retry` 在重试后仍返回 Network 错误；缓存 cluster 不被删除；前端显示错误 toast（"AI 调用失败，已保留上次结果"）；`pipeline_decisions` 记录 `cluster_generation` 错误条目

### 8. 空日（edge case）

- **Given**：2026-05-16 当日无 transcription_history 记录
- **When**：用户切到该日 Day View
- **Then**：后端不调用 LLM，task_clusters 表无该日记录；UI 显示空态文案"今天没有转录"

### 9. 源转录删除（edge case）

- **Given**：cluster A 含 source_history_ids=[10,11,12]，对应 history entries 存在
- **When**：用户在 Dashboard 删除 entry id=11
- **Then**：后端在删除 history 时同步更新所有引用该 id 的 cluster（移除 11）；A 的 source_ids 变为 [10,12]，重算 entry_count=2、total_duration_ms；若 cluster 变空（source_ids=[]）→ 保留 cluster 行不变（不修改 `is_user_modified` 标志，避免系统级动作改写用户意图状态），UI 显示"该 cluster 已失效，建议重新生成"占位

### 10. JSON 解析失败重试（error path）

- **Given**：LLM 返回非法 JSON（缺括号 / 尾随逗号）
- **When**：generate 执行
- **Then**：后端立即用严格模式 retry 一次（prompt 末尾加"仅返回合法 JSON 数组，不要任何额外文字"）；若仍失败，保留旧数据 + 错误 toast；写 pipeline_decisions 错误记录

### 11. Week 视图不跨天聚合（edge case）

- **Given**：一周 5 天每天各有 2-4 个 cluster
- **When**：用户切到 Week View
- **Then**：UI 显示 5 天网格，每天独立显示当天 cluster 卡片；不调用任何跨日 LLM；不出现"周内合并"标题；每张卡保留原 cluster_id

### 12. 👍 不注入 prompt（edge case）

- **Given**：用户给最近 30 天内多个 cluster 添加 👍（无 👎）
- **When**：触发 generate
- **Then**：发送给 LLM 的 prompt USER 部分**不**包含任何 feedback 条目；后端 SQL 仅查 `thumb='down' AND note IS NOT NULL`

### 13. 拆分校验（error path）

- **Given**：cluster A 含 source_history_ids=[10,11,12]
- **When**：用户在 SplitClusterDialog 选中空集合 / 全集 / 含不在 A 中的 id（如 99）
- **Then**：前端按钮禁用 + 提示；若强制调用后端，后端返回 400 错误

### 14. AUX 抽屉折叠/展开（happy path）

- **Given**：用户首次打开 Summary
- **When**：观察 AUX 区域
- **Then**：AUX 默认折叠成单行 chips，cluster 主区占据主视线；点击 chip 展开右侧抽屉显示对应 section；store 中 `auxPanelOpen=true, auxActiveSection=<chip>`；不刷新页面切换 section 时数据不丢失

### 15. Migration 幂等（edge case）

- **Given**：用户更新到新版本，旧 `summaries.stats.task_clusters` JSON 中有 N 条历史 cluster
- **When**：应用启动执行 migration
- **Then**：N 条历史 cluster 写入新 `task_clusters` 表，`is_user_modified=0`、`source_history_ids_json='[]'`；`summaries.stats` 中的 task_clusters 字段被移除；二次启动 migration 不重复插入（用 `INSERT OR IGNORE` 或 migration 版本号控制）

## 性能基准（非强制，仅用于回归发现）

- 单次 generate（50 条 entry）端到端 ≤ 5s（含 LLM 往返）
- 缓存命中读取 ≤ 50ms
- Day View 首次渲染（已缓存）≤ 200ms
- Week View（≤ 50 张卡）渲染 ≤ 300ms

## 实施偏差

> 实施完成后回填。下表记录 24 个任务执行中与原计划的实际差异。

| 原计划                                                                                       | 实际实现                                                                                              | 原因                                                                                                                                                                            |
| -------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `task_clusters` 与 `cluster_feedback` 各自独立 `M::up()` migration                           | 两表 SQL 以 `pub const MIGRATION_SQL` 暴露，由 `history.rs::MIGRATIONS` 集中注册（migration #45/#46） | 调研发现 `llm_metrics.rs:8-9` 明确以 const 形式定义 SQL，主 migration 链由 `history.rs` 单点拥有；多个 `Migrations` 实例对同一 DB 调用 `to_latest()` 会争 `user_version` 元数据 |
| 前端使用 `lucide-react` 图标                                                                 | 使用 `@tabler/icons-react`                                                                            | 项目已大量使用 `@tabler/icons-react`；引入 lucide 会让图标系统分裂                                                                                                              |
| 前端使用 `@/components/ui/Card` 路径别名                                                     | 使用相对路径 `../../../ui/Card`                                                                       | `tsconfig.json` 未配置 `paths` 别名                                                                                                                                             |
| RecapSection 读 `summary.ai_content.recap.{headline,key_progress,...}`                       | 读 `summary.stats.daily_overview`                                                                     | 实际 `Summary` 类型没有 `ai_content` 字段；`daily_overview` 是真实存储位置                                                                                                      |
| ProfileSection 三 tab 渲染结构化对象（vocabulary_stats/expression_stats/time_pattern_stats） | 三 tab 渲染 `string \| null`（LLM 生成的散文）                                                        | `UserProfile` 字段是字符串，不是结构化数据；旧 `AiAnalysisSection` 仅渲染 `style_prompt`                                                                                        |
| `format_hhmm` 用 UTC 毫秒数学 `(secs / 3600) % 24`                                           | 改用 `chrono::Local.timestamp_millis_opt(ms).single()`                                                | T6 code review 指出：用户在 UTC+8 时本地 23:00 渲染为 15:00，破坏 LLM 时段语义                                                                                                  |
| `summary.period_start` 假设是毫秒                                                            | 实际是 Unix 秒                                                                                        | T23 调研：`summary.rs:404` 使用 `start_local.timestamp()` 返回秒；用 `Local.timestamp_opt(s, 0)`                                                                                |
| `summary_id` 与 `entries` 在 orchestrator 内部解析                                           | 移到 T8 command 层解析后传入 `GenerateClustersInput`                                                  | T7 调研发现 `SummaryManager`/`HistoryManager` 没有现成方法；为保持 orchestrator 纯粹，把数据解析推到 command 边界                                                               |
| `SettingsManager::get_settings()` State 注入                                                 | 用 `crate::settings::get_settings(app_handle)` 自由函数                                               | 实际项目没有 `SettingsManager` 类型；settings 通过自由函数获取                                                                                                                  |
| `PromptManager::substitute_variables` 静态方法                                               | 实为 `crate::managers::prompt::substitute_variables` 自由函数                                         | 项目实际定义形态                                                                                                                                                                |
| Cluster 表的 `task_clusters` 字段从 `SummaryStats` 中删除                                    | 字段保留并标 `#[serde(default)]`，Rust 端 struct 重命名为 `LegacyTaskClusterSnapshot`，写时填空数组   | 防御性兼容：旧 summary 行的 JSON 仍能解析，T23 完成后再考虑彻底删除                                                                                                             |
| Cluster 操作（split/merge/cascade）原子性留到 T8-prep 任务                                   | T8 同步实现 `unchecked_transaction()` 包裹                                                            | T8 暴露 commands 后立即需要原子性；早做避免后续被迫修；refactor 增量 < 100 LOC                                                                                                  |
| AI 分析生成 UI（模型选择 + 生成按钮）保留                                                    | T21 中删除，由 cluster regenerate 接管                                                                | 旧 `AiAnalysisSection` 的 generate 逻辑映射到新 cluster 流程；保留会让用户混淆两条 LLM 触发路径                                                                                 |
| `SummaryCalendar` 保留，迁到 AUX                                                             | T21 中删除，未保留                                                                                    | 计划"边界"未明确，新 `PeriodSelector` 用箭头导航代替；若用户需要日历视觉提示，作为 follow-up                                                                                    |
| T8 仅做 commands、T8-prep 单独做原子性                                                       | 合并为一次 T8                                                                                         | 减少 PR 数量、避免重复触碰相同文件                                                                                                                                              |
| `get_history_entries_by_ids` 在 T8 添加                                                      | T13 添加（前端 ClusterDetailDrawer 需要）                                                             | T8 任务列表中未包含此命令；按需补充                                                                                                                                             |
| `extract_json_array` 多 `[...]` 块的鲁棒处理                                                 | 实现"首个 `[` 到最末 `]`"启发式，未加 prose-with-multiple-brackets 防御                               | 主流路径靠 `\`\`\`json` 围栏移除；防御仅供 fallback                                                                                                                             |
| 在 `delete_unmodified_for_date` 后失败仍要保留旧数据                                         | 实现先取数据、再 LLM、再 delete + insert；LLM 失败时不动 DB                                           | 一致：失败降级保留旧数据；只有 sanitize 后才进入 delete-then-insert                                                                                                             |
| 计划中 plan 全程假设 `crate::types::AppSettings`                                             | 实际在 `crate::settings::AppSettings`                                                                 | T7 调研后修正 import 路径                                                                                                                                                       |
| 旧 stats.task_clusters JSON 迁移后清空                                                       | 不清空，保留为只读历史                                                                                | `LegacyTaskClusterSnapshot` 反序列化路径还在用；清空会破坏读 compat。代价：用户删除所有 migrated cluster 后重启会被重新插回                                                     |
| 前端单元测试                                                                                 | 不写（项目未配置 Vitest）                                                                             | 依靠 `tsc --noEmit` 类型检查 + T24 手动验收                                                                                                                                     |
| 模板条件块 `{{#block}}...{{/block}}` 通用支持                                                | T7 实现专用 `strip_conditional_block` 辅助                                                            | `substitute_variables` 仅做 flat 替换；专用 helper 是最小侵入                                                                                                                   |
