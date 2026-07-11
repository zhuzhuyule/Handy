You are analyzing user corrections to speech-to-text output. Determine whether each correction fixes an ASR misrecognition or is an intentional semantic edit.

## Corrections

{{corrections}}

{{existing_hotwords}}

## Phonetic similarity guide

ASR errors almost always involve words that sound similar. Use these patterns to identify them:

**English**: similar spelling, shared first letter, similar vowel patterns, consonant substitution (e.g. "Agent"→"Aigne", "Brolet"→"blocklet", "great"→"grate", "affect"→"effect")

**Chinese**: similar pinyin — front/back nasal confusion (an/ang, en/eng, in/ing), similar initials (b/p, d/t, g/k, z/zh, c/ch, s/sh, n/l, f/h), similar finals (e.g. "按门"→"安门", "八哥"→"八戒", "机器"→"激励")

If the original and corrected forms share phonetic similarity by any of the above patterns, it is very likely an ASR error.

## Classification

**asr_error** — The user is correcting a misheard word/phrase:

- Homophones, near-homophones, or phonetically similar words
- Proper nouns/brands/terms misrecognized as common words (ASR favors common words over rare ones)
- English words with wrong spelling/casing from dictation (e.g. "chat gpt"→"ChatGPT")
- A common word replaced by an uncommon/unknown word — strongly suggests restoring an intended proper noun

**semantic_edit** — The user is intentionally rewording or rephrasing, not correcting a mishearing. The original and corrected forms have clearly different meanings and no phonetic relationship.

Default to `asr_error` when the corrected form appears to be a proper noun, brand, or domain-specific term. Only classify as `semantic_edit` when the change is clearly about meaning, not pronunciation.

## Output

JSON array only, no other text:

[{ "original": "A", "corrected": "B", "type": "asr_error", "category": "term" }]

- `type`: "asr_error" | "semantic_edit"
- `category` (asr_error only): "person" | "brand" | "term" | "abbreviation"
