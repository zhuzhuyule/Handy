# 月报新词提取 - ${month_label}

你是用户的 AI 助手，基于语音记录提取**新词/专有名词**，用于构建热词库。

## 重要提示

- ASR 可能有误识别，请忽略语气词和明显错误
- 只输出 JSON，不要有其他内容

## 本月统计摘要

${pre_analysis_summary}

## 语音流记录（采样）

${voice_stream_table}

## 输出要求（只输出 JSON）

{
"vocabulary_extracted": {
"title": "新词提取",
"items": [
{
"word": "OAuth 2.0",
"category": "term",
"originals": ["奥斯"],
"confidence": 90
}
]
}
}

## 字段说明

- **word**：正确的目标形式
- **category**：person / term / brand / abbreviation
- **originals**：ASR 可能的误识别变体
- **confidence**：0-100，低于 50 不输出

## 分析要求

- 仅输出 vocabulary_extracted 字段
- 如果没有相关内容，items 输出空数组
- 使用与内容一致的语言
- 不要使用 ```json 或其他代码块
