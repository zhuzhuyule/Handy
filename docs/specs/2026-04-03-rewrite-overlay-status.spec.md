---
name: "Rewrite Overlay 状态增强"
tags: [overlay, rewrite, ux]
depends_on: []
estimate: "2h"
---

## 意图

"当用户在 review 窗口通过语音改写文档时，overlay 应展示改写次数徽标和动态状态文案，让用户清楚知道当前是第几次修改、系统正在做什么。"

当前问题：

1. rewrite 模式下 overlay 因 `main.tsx` payload 解析 bug 全程黑屏
2. 即使修复 bug，现有文案仅显示"第 N 次修改"，缺少转录/处理阶段的动态状态反馈
3. LLM 返回的 operation 类型（改写/扩充/润色等）未向用户展示

## 约束

- overlay 宽度 208px（无 realtime text 时），空间有限
- overlay 是 NSPanel（macOS），事件通过 `overlay_window.emit()` 发送
- `RewriteResponse.operation` 字段已存在，值为 rewrite/expand/format/translate/polish/append
- `rewrite_count` 由后端 `REWRITE_COUNT` 原子计数器管理，review 窗口开启时归零
- 现有 `emit_rewrite_overlay_state_with_retry` 发送对象 payload `{ state, rewrite_count }`

## 已定决策

1. **数字徽标替换图标**：rewrite 模式下，左侧麦克风/转录图标替换为灰色圆形徽标 + 白色数字。原因：overlay 空间小，叠加角标会拥挤；用户在 review 窗口已知在做改写，不需要麦克风图标提示。

2. **统一处理状态文案**：转录完成后 LLM 处理期间显示"处理中…"，不区分子阶段。原因：改写是单次 LLM 调用，无需细分。

3. **完成闪现 operation 文案**：LLM 返回后从 response 提取 operation，overlay 短暂展示（~800ms）对应文案后隐藏。原因：给用户"做了什么"的确认反馈。

4. **修复 main.tsx payload 解析 bug**：`show-overlay` 监听器需正确解析对象 payload，提取 `.state` 字段。

## 边界

### 允许修改

- `src/overlay/main.tsx` — 修复 payload 解析
- `src/overlay/RecordingOverlay.tsx` — 徽标渲染、完成态、动画
- `src/overlay/RecordingOverlay.css` — 徽标样式
- `src/i18n/locales/zh/translation.json` — 新增 operation 文案
- `src/i18n/locales/en/translation.json` — 新增 operation 文案
- `src-tauri/src/overlay.rs` — 完成态延迟隐藏逻辑
- `src-tauri/src/actions/transcribe.rs` — 提取 operation 并发送事件
- `src-tauri/src/actions/post_process/mod.rs` — 如需导出 RewriteResponse 类型

### 禁止

- 不修改 `system_votype_rewrite.md` prompt 文件
- 不改变 LLM 调用流程（不增加额外请求）
- 不修改 review 窗口逻辑

## 排除范围

- 非 rewrite 模式的 overlay 改动（普通录音/转录保持现状）
- 意图预判分步请求（当前单次 LLM 调用不变）
- operation 文案的多语言全量翻译（仅做 zh + en，其他语言后续补充）

## 验收场景

### 1. rewrite_overlay_shows_count_badge

- **Given**: review 窗口已打开，用户第 2 次按下录音键
- **When**: overlay 弹出进入录音状态
- **Then**: 左侧显示灰色圆形徽标内数字"2"，中间显示波形动画

### 2. rewrite_overlay_status_transitions

- **Given**: 用户在 review 窗口录音并停止
- **When**: 经历转录 → LLM 处理 → LLM 返回各阶段
- **Then**: overlay 依次显示"转录中…"→"处理中…"→"已改写"（或对应 operation），最后一步保持 ~800ms 后隐藏

### 3. rewrite_overlay_operation_mapping

- **Given**: 用户说"把这段话扩充一下"
- **When**: LLM 返回 `operation: "expand"`
- **Then**: overlay 短暂显示"已扩充"+ 徽标数字，~800ms 后隐藏

### 4. rewrite_overlay_bug_fix_no_black_screen

- **Given**: review 窗口已打开
- **When**: 用户按下录音键并停止录音
- **Then**: overlay 在所有阶段均正常显示内容，不出现黑屏

### 5. rewrite_overlay_first_rewrite

- **Given**: review 窗口刚打开（rewrite_count = 0），用户首次按录音键
- **When**: overlay 弹出
- **Then**: 徽标显示"1"

### 6. normal_recording_unaffected

- **Given**: 用户在普通应用中按录音键（非 review 窗口）
- **When**: overlay 弹出
- **Then**: 左侧显示原有麦克风/转录图标，无数字徽标，行为与改动前一致

## 实施偏差

> 功能完成后填写。

| 原计划 | 实际实现 | 原因 |
| ------ | -------- | ---- |
| —      | —        | —    |
