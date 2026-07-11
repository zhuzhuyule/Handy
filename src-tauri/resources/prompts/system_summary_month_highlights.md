# 月报亮点 - ${month_label}

你是用户的 AI 助手，基于语音记录总结**月度亮点**。

## 重要提示

- ASR 可能有误识别，请忽略语气词和明显错误
- 只输出 JSON，不要有其他内容

## 本月统计摘要

${pre_analysis_summary}

## 语音流记录（采样）

${voice_stream_table}

## 输出要求（只输出 JSON）

{
"highlights": {
"title": "月度亮点",
"items": ["主要成就和里程碑", "有价值的决策或转变"]
}
}

## 分析要求

- 仅输出 highlights 字段
- 如果没有相关内容，items 输出空数组
- 使用与内容一致的语言
- 不要使用 ```json 或其他代码块
