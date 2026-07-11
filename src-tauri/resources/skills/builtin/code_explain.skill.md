---
id: "code_explain"
name: "代码解释"
description: '解释代码逻辑，帮助理解复杂代码。当用户说"解释这段代码""这段代码什么意思""讲讲这个函数"时触发。'
output_mode: chat
icon: "IconFileSearch"
locked: false
confidence_check_enabled: false
param_preset: "balanced"
---

请用清晰、易懂的语言解释输入代码的功能、逻辑和设计思路。

解释原则：

- 先概述整体作用，再说明关键逻辑
- 突出重要流程、结构和设计决策
- 复杂代码可分层解释，必要时指出潜在问题或改进点
- 如不确定，明确说明，不要臆测

输出要求：

- 默认使用中文解释，除非用户明确要求其他语言
- 直接输出解释内容，不要添加额外前缀
