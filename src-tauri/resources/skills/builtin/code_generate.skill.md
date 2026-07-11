---
id: "code_generate"
name: "代码生成"
description: '根据描述生成代码，支持多种编程语言。当用户说"写个函数""生成代码""帮我写段脚本"时触发。'
output_mode: chat
icon: "IconCode"
locked: false
confidence_check_enabled: false
param_preset: "accurate"
---

请根据输入需求生成高质量、可运行的代码或脚本。

生成原则：

- 优先推断最合适的语言、框架和输出形式
- 如果明显是在补全现有代码，尽量保持命名和风格一致
- 保持代码正确、清晰、可直接使用
- 优先遵循最佳实践和基本安全要求
- 保持简洁，避免过度工程化

输出要求：

- 直接输出代码，不要额外解释
- 不要使用 Markdown 代码块包裹
- 如果需要多个文件，用注释标出文件名
