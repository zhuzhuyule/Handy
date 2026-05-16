# Summary Cluster Refactor — 手动验收清单

> 配套 `2026-05-15-summary-cluster-refactor.spec.md` 中 15 条 BDD 场景。所有自动化检查（24 任务的 cargo test、tsc）已通过；本清单覆盖只能 UI 层确认的部分。

## 前置准备

```bash
# 1. 确认编译通过
cd src-tauri && rtk cargo check
rtk bun run tsc --noEmit

# 2. 启动 dev build
rtk bun tauri dev
```

确认你的 Settings → Models 已选好 post-process provider + model（cluster 生成会调用这套配置）。

## 准备一份测试数据

```bash
# macOS 路径
DB="$HOME/Library/Application Support/com.handy.app/handy.db"

# 看最近 7 天每天的转录条数（选数据最多的一天作为主测试日）
rtk sqlite3 "$DB" "
SELECT date(timestamp/1000, 'unixepoch','localtime') AS d, COUNT(*) AS n
FROM transcription_history
WHERE deleted=0
GROUP BY d
ORDER BY d DESC
LIMIT 7;
"
```

记下数据最多的那天作为 **TEST_DATE**。

---

## 场景 1：首次生成（happy path）

- [ ] 打开 Sidebar → Summary（Ctrl+2）
- [ ] PeriodSelector 切到 **TEST_DATE**
- [ ] DayView 显示「AI 生成中...」短暂闪过，然后呈现 3-8 个 cluster 卡片
- [ ] 每张卡显示：标题、状态徽章、时长 · 条目数、keywords chip、可选 summary、可选 blockers 横条、next_step 行

DB 验证：

```bash
rtk sqlite3 "$DB" "SELECT date, COUNT(*) FROM task_clusters WHERE date='TEST_DATE' GROUP BY date;"
```

预期：返回一行，count > 0。

## 场景 2：缓存命中

- [ ] 同一会话内，切到别的日期再切回 **TEST_DATE**
- [ ] 不再看到「AI 生成中...」flash；cluster 直接显示
- [ ] 后端控制台无新的 LLM 调用日志（可在 dev console 看 fetch/网络）

## 场景 3：重命名 → 重新生成保护

- [ ] 在某个 cluster 上 hover 标题旁的铅笔图标 → 改名为「OAuth 调试」→ Enter
- [ ] 卡片右侧出现「已编辑」紫色徽章
- [ ] 点顶部 ⟳重新生成按钮
- [ ] 重新生成后，「OAuth 调试」cluster 仍然存在且标题未变
- [ ] 其他原 AI-generated cluster 可能改变了标题/内容

DB 验证：

```bash
rtk sqlite3 "$DB" "SELECT title, is_user_modified, user_modified_fields FROM task_clusters WHERE date='TEST_DATE';"
```

预期：编辑过的那行 `is_user_modified=1`，`user_modified_fields` 含 `"title"`。

## 场景 4：反馈注入下次 prompt

- [ ] 在某 cluster 卡片右下角点 👎 按钮
- [ ] popup 弹出，输入备注「Slack 消息应该单独成簇」→ 提交
- [ ] 切到次日，触发首次生成（或回到当日按 ⟳重新生成 + 力 force）
- [ ] （可选）在 dev console 加 `eprintln!` 打印 prompt body，确认 USER FEEDBACK 块含该备注

AUX → Feedback History 应该列出该 👎 记录。

## 场景 5：拆分

- [ ] 选一个 source_history_ids ≥ 5 的 cluster
- [ ] 点「🔀拆分」
- [ ] Dialog 弹出：勾选 2-3 个条目，填新标题「X」→「拆分」
- [ ] 列表刷新：出现新 cluster「X」；原 cluster 的 entry_count 与 duration 减少
- [ ] 两张卡都有「已编辑」徽章

## 场景 6：合并

- [ ] 选两个 cluster A 和 B
- [ ] 在 A 上点「🔗合并」
- [ ] Dialog 中勾选 B → 合并
- [ ] B 消失；A 的 entry_count 增加；A 标题不变；A「已编辑」

## 场景 7：网络错误降级

- [ ] 断开 Wi-Fi（或在 Settings 中故意把 API key 改错）
- [ ] 点 ⟳重新生成
- [ ] Sonner toast 弹出「AI 调用失败，已保留上次结果」
- [ ] 现有 cluster 不消失

## 场景 8：空日

- [ ] PeriodSelector 切到一个**无任何转录**的日期
- [ ] 不弹「AI 生成中...」；直接显示「今天没有转录」空态
- [ ] DB 中该日期没有 task_clusters 行

## 场景 9：源转录删除级联

- [ ] 记下某 cluster 的 source_history_ids（展开看时间线，记一个 entry_id）
- [ ] 切到 Dashboard（Ctrl+1）→ 找到该 entry → 删除
- [ ] 回到 Summary → 该 cluster 的 entry_count 减一
- [ ] 时间线中该条目消失

DB 验证：

```bash
rtk sqlite3 "$DB" "SELECT source_history_ids_json, entry_count FROM task_clusters WHERE id='<the-cluster-id>';"
```

## 场景 10：JSON 解析失败重试

依赖 LLM 输出。本场景由 T6 单元测试覆盖（9 测试中 `test_parse_llm_output_invalid_returns_err`）。本次手动验证只需确认：

- [ ] 长时间运行后 `pipeline_decisions` 日志中没有大量 parse-error 记录
  ```bash
  rtk sqlite3 "$DB" "SELECT COUNT(*), error_type FROM pipeline_decisions WHERE timestamp > strftime('%s','now','-7 days')*1000 GROUP BY error_type;"
  ```

## 场景 11：Week 视图不跨天

- [ ] PeriodSelector 切到 week
- [ ] 7 天网格呈现
- [ ] 每天独立卡片，无"跨日合并"或"周内同主题汇总"
- [ ] 顶部「本周热点」chip 云出现（基于关键词重合）

## 场景 12：👍 不注入

- [ ] 给某 cluster 点 👍（不填备注或填正面备注）
- [ ] 触发 ⟳重新生成
- [ ] AUX → Feedback History 中没有该 👍 记录（界面只列 👎+note）
- [ ] （可选）确认 prompt body 中无任何 USER FEEDBACK 行

## 场景 13：拆分校验

- [ ] 打开 SplitClusterDialog
- [ ] 不勾选任何条目 → 「拆分」按钮 disabled，文末提示「至少选一条」
- [ ] 勾选全部 → 「拆分」disabled，文末提示「不能全选」
- [ ] 部分勾选 + 空标题 → disabled

## 场景 14：AUX 抽屉折叠/展开

- [ ] DayView 顶部看到「▸ AUX」标签和 6 个 chip：Stats / Recap / Profile / Hotword / Export / Feedback History
- [ ] 点击 Stats chip → 右侧抽屉滑入，显示 4 张统计卡
- [ ] 抽屉内顶部 6 个二级 chip 可切换 section（Stats → Recap → Profile…）
- [ ] 切换 section 时数据保留，不会刷新
- [ ] 点 X 关闭 → chip 行保留

## 场景 15：Migration 幂等

如果你有 pre-T22 时期的旧 summary 行：

```bash
# 启动前查询旧 stats.task_clusters JSON
rtk sqlite3 "$DB" "
SELECT id, period_start, json_extract(stats, '\$.task_clusters') AS legacy_clusters
FROM summaries
WHERE stats LIKE '%task_clusters%'
LIMIT 5;
"
```

- [ ] 启动 app
- [ ] 后台异步 migration 运行（看 stderr 日志，若有错误会打印）
- [ ] DB 查询 `task_clusters` 表，应能看到迁移过来的行：
  ```bash
  rtk sqlite3 "$DB" "
  SELECT date, COUNT(*) AS n FROM task_clusters
  WHERE is_user_modified=0 AND source_history_ids_json='[]'
  GROUP BY date;
  "
  ```
- [ ] 重启 app
- [ ] 上面那个查询的 count 不变（不重复插入）

---

## 完成后

回到此清单顶部，确认所有勾选项已 ✅。如有失败项，在 `spec` 的「实施偏差」表中追加一行，记录症状和决策。
