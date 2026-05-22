---
name: "App 规则推荐引擎"
tags: [post-processing, history, app-profile, suggestion, overlay]
depends_on:
  - "AppProfile / TitleRule 已存在 (settings.rs:548)"
  - "transcription_history 已记录 app_name / window_title / post_process_prompt_id"
  - "现有 overlay (ManagedPendingSkillConfirmation) 通路可扩展"
estimate: "2-3 days"
---

## 意图

"在用户在某个 `(app, title)` 上下文中**用同一个 prompt 录了 5 次**且尚未为该上下文配置 TitleRule 时，弹一个悬浮窗询问"是否为该窗口添加规则"。用户接受则自动写入一条 `TitleMatchType::Exact` 的 TitleRule（落到对应 AppProfile）。用户拒绝则在 10/20/40 倍增阈值时再提示一次；选择"别再问"则永久静默。"

解决的问题：votype 已有完整的 AppProfile + TitleRule + pipeline 应用基础设施，但用户必须主动进入设置 → AppRules 面板才能配规则。绝大多数用户不会注意到这条配置路径，错失"在常用上下文里走更合适的 prompt"这一价值。引擎把"该不该配规则"的判断自动化，把决策点搬到产品自然出现的弹窗里。

## 约束

- 信号源**只用** `transcription_history` 现有列（`app_name`, `window_title`, `post_process_prompt_id`），不引入新的运行时计数表。频率检查走 SQL `COUNT() GROUP BY`。
- 计数维度严格为 `(app_name, window_title, post_process_prompt_id)` 三元组。只在同一 prompt 反复使用时才视为"用户偏好已稳定"。
- 自动写入的 TitleRule 使用 `TitleMatchType::Exact`（V1 新增变体），避免 substring 误触。手工 UI 创建的 TitleRule 也默认 Exact。
- 触发时机：**文本已成功 paste 到目标 app 之后**（不打断 review / 不阻塞 recording 流程）。
- 弹窗复用 votype 现有 overlay 通路（`ManagedPendingSkillConfirmation` 那条），不新增独立 webview。
- 不修改 `AppProfile` / `TitleRule` 已有字段；只**新增** `TitleMatchType::Exact` 变体 + 一个 SQLite 表跟踪建议状态。
- 不修改 routing 决策路径——`override_prompt_id` 早已支持 AppProfile 注入，引擎只往里写规则、不参与 routing。
- 触发节流：单次录音回合（单次 stop()）最多触发**一次**弹窗，避免连续命中多个阈值时多窗叠加。
- 遵守 CLAUDE.md 运行时规则：任何后台任务用 `tauri::async_runtime::spawn`，不要在协调线程 block_on。
- 修改前提交前消除所有 warning。

## 已定决策

- **计数维度 = `(app, title, prompt_id)` 三元组。** 不选 `(app, title)` 二元组：5 次录音里用了 3 个不同 prompt → 没有偏好可推荐。三元组保证只有"用户已经稳定选择了某个 prompt"时才提示。

- **阈值梯度 = 5 / 10 / 20 / 40，倍增。** 不选 3/6/12/24（用户最初提议）：5 起步噪声更低，倍增节奏给用户充分的"再用一阵看看"窗口而不会频繁打扰。

- **触发时机 = 文本 paste 到目标 app 之后。** 不选"写完 history 行之后"：会在 review 流程中弹窗，打断用户思路。paste 完成意味着用户已经看到了结果、回到了原始 app，此时弹一个非阻塞悬浮窗代价最低。

- **TitleMatchType 新增 `Exact` 变体。** 不选"用 Regex `^...$`"：自动生成的规则若用 regex，title 中的特殊字符（`|`、`:`、`#`、`/`）需要逐个 escape；用户在 UI 手工查看时也会被 regex 语法吓退。`Exact` 是字面 == 比对，零歧义。

- **弹窗复用 overlay 通路。** 不新建 webview：现有 `ManagedPendingSkillConfirmation` 通路已经支持"出现一个 overlay、三选项响应、关闭"。新增一个 pending kind `RuleSuggestion`，复用现有渲染框架。

- **三选项 UX：接受 / 这次不要 / 别再问。** "这次不要"= 当前阈值已问过、下一阈值再问；"别再问" = 永久静默该 (app, title)。不选"二选项 (接受/拒绝)"：拒绝时的语义模糊（一次性 vs 永远）容易让用户后悔。

- **节流：单次 stop() 最多触发一次弹窗。** 极端情况：在 batch 录入或快速连录时计数可能在同一回合里跨过多个阈值。只对当次插入后 ROW 计算的 count 检查最高一个阈值。

- **建议状态表 `app_rule_suggestions` 用 SQLite 而非 settings.json。** 多行、按 `(app, title)` 唯一键查询，SQLite 自然合适；settings.json 用于全局配置、不适合作日志型表。

- **新增 `respond_rule_suggestion(decision, app, title, prompt_id)` Tauri command。** 接收前端三选项响应，统一写入：accepted → 调用 `upsert_app_profile` 追加 TitleRule + 写 suggestions 表记录；dismissed / never_again → 仅写 suggestions 表。

- **若 (app) 还没有 AppProfile，引擎自动为该 app 创建一个新 AppProfile 后再追加 TitleRule。** 不要求用户先在设置里手工建 profile——这违背"零额外操作"的初衷。新建的 AppProfile 沿用全局默认 policy。

- **首次发版后已有的历史数据也参与计数。** 不引入"只看本特性上线之后的录音"的窗口——用户上线后第一次触发可能命中 5/10/20/40 中任一阈值。这是优点：能立刻产出建议，而不是要再等 5 次新录音。

- **同一 (app, title) 即便 prompt 不同也只算一次"是否已配置规则"的检查。** 若该 app 的 AppProfile 中存在**任一** TitleRule（不论 `match_type` 是 Exact / Text / Regex）能对当前 title 求值为"匹配"，无论其 prompt_id 是什么，引擎都不再为该 (app, title) 推荐。覆盖现有规则的需求由用户手工去设置里改，引擎不参与。

## 边界

### 允许修改

- 新建：
  - `src-tauri/src/managers/suggestion_engine.rs`：核心引擎模块（信号检测 + 决策落库）
- 修改：
  - `src-tauri/src/settings.rs`：`TitleMatchType` 加 `Exact` 变体；保留 `Text` 和 `Regex` 兼容旧值
  - `src-tauri/src/managers/history.rs`：加 migration 建 `app_rule_suggestions` 表
  - `src-tauri/src/managers/mod.rs`：注册新 `suggestion_engine` 模块
  - `src-tauri/src/actions/transcribe.rs`：在每条 paste 完成后（line 2454 / 2767 / 2944 三处之后）调用 `suggestion_engine::check_after_paste(history_id)`
  - `src-tauri/src/shortcut/settings_cmds.rs`：新增 `respond_rule_suggestion` command
  - `src-tauri/src/lib.rs`：注册新 command + 新 mod
  - `src-tauri/src/review_window.rs` 或 overlay 模块（具体位置由 plan 确定）：扩展 pending kind 加 `RuleSuggestion { app, title, prompt_id, prompt_name, count, threshold }`
  - `src/components/overlay`（具体路径由前端调研确定）：增加 RuleSuggestion 渲染态
  - `src/components/settings/post-processing/AppReviewPolicies.tsx`：TitleMatchType `SegmentedControl` 加 `Exact` 选项；自动创建的规则默认显示 Exact
  - `src/bindings.ts`：specta 重生成
  - `src/i18n/locales/*/translation.json`：新增弹窗文案 key（接受 / 这次不要 / 别再问 / 主提示）

### 禁止

- 修改 `AppProfile` / `TitleRule` 已有字段（仅扩 enum 变体）
- 修改 `routing.rs` / `extensions.rs` 的 `override_prompt_id` 路径
- 修改 `AppProfilesManager` 主体 UX（仅在 TitleMatchType 选项条增加一个变体）
- 在 recording 或 review 流程的任何路径里弹本特性的窗口
- 引入新的 webview / 新的 toast 库——弹窗只能用现有 overlay
- 同步阻塞 paste 路径——`check_after_paste` 必须是非阻塞（`async` 或 spawn）

## 排除范围

- 推荐"覆盖现有规则" —— 已存在的 TitleRule 不被引擎修改。
- 推荐"删除某条用户手配的规则"。
- 对 (app, title) 做模糊去重（例如把 "Slack | #a" 和 "Slack | #b" 合并 group）。
- 时间窗口（"最近 N 天的数据"）—— 累计计数即可；上线初期能立刻借助历史数据弹有用建议。
- 多 prompt 候选（"你 3 次用 Polish 2 次用 PassThrough，要哪个？"）——三元组维度天然保证单一 prompt。
- 在弹窗里允许用户调整 prompt——只是"接受 / 拒绝该建议"。要改 prompt 让用户去 AppProfilesManager。
- 历史数据回填的"补刷"机制——上线时不主动跑全表扫描；每次新 paste 完成时检查当前 (app, title, prompt_id) 触发即可。
- 撤销已自动创建的 TitleRule——用户去 AppProfilesManager 里删。

## 验收场景

### 1. happy_path_suggest_and_accept

- **Given**: 用户在 `app="Slack", title="Slack | #project-alpha"` 中已用 `Polish` prompt 录过 4 次。历史表中三元组 `(Slack, "Slack | #project-alpha", polish_skill_id)` count = 4。`app_rule_suggestions` 中无对应行。AppProfile 中没有命中该 `(app, title)` 的 Exact TitleRule。
- **When**:
  1. 第 5 次录音完成、文本 paste 到 Slack 之后
  2. `suggestion_engine::check_after_paste(history_id=N)` 触发
  3. 计算得当前三元组 count = 5，命中阈值 5
- **Then**:
  - overlay 弹窗，文案包含 "Slack" / title 截断显示 / "Polish" / "5 次"
  - 三个按钮：接受 / 这次不要 / 别再问
  - 用户点"接受" → `respond_rule_suggestion(Accepted, ...)` 被调用
  - AppProfile 中追加一条 `TitleRule { pattern: "Slack | #project-alpha", match_type: Exact, prompt_id: polish_skill_id }`（若该 app 无 AppProfile，自动建一个）
  - `app_rule_suggestions` 中插入行 `(Slack, title, threshold=5, decision='accepted', decision_at=now)`
  - 弹窗关闭，不影响后续行为
  - 第 6 次录音在同一 (app, title) → routing 走 AppProfile.rules → 自动用 Polish

### 2. edge_case_mixed_prompts_no_trigger

- **Given**: 用户在 Slack 同一 title 中录了 5 次：4 次 Polish + 1 次 PassThrough
- **When**: 第 5 次（PassThrough）paste 完成后引擎检查
- **Then**:
  - 三元组 `(Slack, title, polish_skill_id)` count = 4，未达 5
  - 三元组 `(Slack, title, passthrough_sentinel)` count = 1，未达 5
  - 不弹窗
  - `app_rule_suggestions` 无变更

### 3. edge_case_existing_rule_skip

- **Given**: AppProfile.rules 已有任意一条 TitleRule 能对当前 title 求值为"匹配"（举例 A：`{ pattern: "Slack | #project-alpha", match_type: Exact }`；举例 B：`{ pattern: "Slack", match_type: Text }`——Text 是 substring，"Slack" 在 title 中出现也算匹配；举例 C：`{ pattern: "^Slack.*", match_type: Regex }`）。三元组某 prompt count 已达 5。
- **When**: paste 完成后引擎检查
- **Then**:
  - 引擎按 `match_type` 对当前 title 做匹配测试，任一规则匹配成功即跳过推荐
  - 不弹窗
  - 不写 `app_rule_suggestions`

### 4. happy_path_dismiss_then_retrigger

- **Given**: 三元组 count = 5，用户点了"这次不要"。`app_rule_suggestions` 写入 `(app, title, threshold=5, decision='dismissed')`。
- **When**:
  1. 用户继续在同一 (app, title) 用同一 prompt 录到 count = 10
  2. paste 完成后引擎检查
- **Then**:
  - 引擎读 suggestions 表见 `decision='dismissed', threshold=5 < 10`
  - 命中阈值 10，再次弹窗
  - 文案中 count 显示 10
  - 若用户再点"这次不要"，suggestions 行更新为 `threshold=10, decision='dismissed'`
  - 直到 20 / 40 才再问

### 5. happy_path_never_again_silenced_forever

- **Given**: 用户在 count=5 时点了"别再问"。`app_rule_suggestions` 中 `(app, title, threshold=5, decision='never_again')`。
- **When**: 用户继续在同一 (app, title) 录到 10 / 20 / 40 / 80 / ...
- **Then**:
  - 引擎读 suggestions 表见 `decision='never_again'`，直接跳过
  - 永远不弹窗（直到用户手工去删除 suggestions 行 / 后续版本提供清除入口）

### 6. edge_case_empty_title_skip

- **Given**: 用户录音时 `fetch_active_window()` 拿到 `title=""`（Wayland / 某些 fullscreen 应用）或 `title=None`
- **When**: paste 完成后引擎检查
- **Then**:
  - 引擎跳过该 history 行（title 为空 / null 一律不参与计数）
  - 不弹窗、不写 suggestions

### 7. edge_case_single_dispatch_per_stop

- **Given**: 出于某种边缘原因（如修复历史数据），同一次 stop() 写完 history 后引擎检查时发现 count 从 4 直接跳到 11（同时跨过 5 和 10 阈值）
- **When**: paste 完成后引擎检查
- **Then**:
  - 仅弹**一次**窗口，按更高阈值（10）显示
  - 不出现两个弹窗叠加
  - suggestions 行记录 `threshold=10`

### 8. happy_path_auto_create_app_profile

- **Given**: AppProfile 列表里**根本没有** "Slack" 的 profile（用户从未为 Slack 配过任何东西）。三元组 count = 5。
- **When**: 用户点"接受"
- **Then**:
  - 引擎调用 `upsert_app_profile`，创建新 AppProfile `{ id: uuid, name: "Slack", policy: Auto, prompt_id: None, rules: [TitleRule { ... }] }`
  - settings.app_profiles 中多出一个 entry
  - 第 6 次录音 routing 命中新 profile.rules[0]

### 9. edge_case_title_match_type_migration

- **Given**: 用户的本地 settings.json 中存在旧版 TitleRule，`match_type` 字段为 `"text"`（旧版 Text/Regex 二选一）
- **When**: 新版本启动、settings 反序列化
- **Then**:
  - 旧 `"text"` 反序列化为 `TitleMatchType::Text`，含义不变（substring 比对）
  - 旧 `"regex"` 反序列化为 `TitleMatchType::Regex`，含义不变
  - 新 `"exact"` 是 V1 新增，旧版本不会写出
  - 测试覆盖：构造一个含 `"text"` 和 `"regex"` 的 settings.json，反序列化、再序列化，roundtrip 不改变值

## 实施偏差

| 原计划                                                                          | 实际实现                                                                                                                                                                   | 原因                                                                                                                                                               |
| ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `resolve_prompt_name` 读 `settings.skills`                                      | 实际读 `settings.post_process_prompts`（项目中 Skill 列表的字段名）                                                                                                        | spec/plan 写时假设的字段名与代码实际不一致，实现时按代码现状改                                                                                                     |
| 前端只需更新 `src/bindings.ts` 即可让 TS 识别 `"exact"`                         | 同时改 `src/lib/types.ts` 中的 Zod `TitleMatchTypeSchema` 枚举                                                                                                             | `AppReviewPolicies.tsx` 导入的是 Zod 推断版而非 specta 生成版；不补 Zod 枚举会 TS2367                                                                              |
| Task 10 仅 `AppReviewPolicies.tsx` 改一处                                       | 增加了 test-match 分支：`else if (rule.match_type === "exact") { ... }` 使用 case-insensitive equality                                                                     | 现有 "Test Match" 预览特性原本只覆盖 text/regex 两支，加 exact 后必须三支齐全才能正常预览                                                                          |
| 三处 paste 站点直接调用 `utils::paste(text, ah_inner)` 后接 `check_after_paste` | `utils::paste` 按值消耗 `AppHandle`，需要在调用前 `ah.clone()` 出一份给 `check_after_paste` 用                                                                             | spec 检查时未注意 paste 函数签名；实现时按所有权调整                                                                                                               |
| Task 1 仅改 `settings.rs`                                                       | 同时扩展了 `transcribe.rs` (2 处) 和 `history.rs` (1 处) 的 `TitleMatchType` match 表达式                                                                                  | 增加 enum 变体会触发非穷尽 match 编译错误；按"对 Exact 走 literal equality"的语义补充                                                                              |
| Task 8 plan 假定 `cargo build` 触发 specta 重生成                               | 仍需启动 `bun tauri dev` 30 秒后 kill 才会真正触发 specta export（与 custom-add-model spec 相同）                                                                          | specta export 在 `pub fn run()` 内 `#[cfg(debug_assertions)]`，非 build-script                                                                                     |
| Task 7 commit 应"无 Claude Code 署名 footer"                                    | 实际 `dd945d71` commit 含 `Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>`                                                                                      | 实现 subagent 的 git 配置默认追加 footer；后续 commit 已纠正                                                                                                       |
| `record_decision` UPSERT 用 `ON CONFLICT(app_name, title)`                      | 实现保持一致，但 Task 3 reviewer 指出 `.ok()` 静默 swallow 非 NoRows 错误；Task 3 fix-up commit `dc7e0601` 改为显式 match，仅 NoRows → None，其他 propagate                | 防止 DB lock 等真实错误被静默成"无 prior decision"导致重复弹窗                                                                                                     |
| Spec §决策"自动写入用 Exact"对手工 UI 默认无要求                                | UI 中新加的 SegmentedControl `Exact` 选项不是默认；用户在 UI 创建 TitleRule 时仍默认沿用之前的 `Text`                                                                      | spec 仅约束**自动写入**的默认值，手工 UI 沿用既有默认更稳                                                                                                          |
| spec §决策"复用 overlay 通路，三选项 UX：接受 / 这次不要 / 别再问"              | 改用原生 OS 确认对话框（tauri-plugin-dialog `YesNoCancelCustom`）替代 overlay 内 3-按钮卡。recording overlay 仍按原流程在 paste 后隐藏；建议对话框作为独立焦点窗口短暂出现 | 用户测试反馈：overlay 内嵌建议卡和录音 overlay 内容叠加显示不直观；原生对话框天然焦点抓取、隔离背景、关闭窗口默认即 Cancel→Dismissed，更符合"消失即视为不要"的语义 |
