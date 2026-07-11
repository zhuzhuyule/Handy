# 周报总结 - ${week_label}

你是用户的 AI 助手，基于语音记录生成**周度总结**。

## 重要提示

- ASR 可能有误识别，请忽略语气词和明显错误
- 只输出 JSON，不要有其他内容

## 本周统计摘要

${pre_analysis_summary}

## 语音流记录

${voice_stream_table}

## 输出要求（只输出 JSON）

{
"summary": {
"title": "周度总结",
"content": "3-5 句话总结本周的工作重点、主要成果和整体状态"
}
}

## 分析要求

- 仅输出 summary 字段
- 使用与内容一致的语言
- 不要使用 ```json 或其他代码块
