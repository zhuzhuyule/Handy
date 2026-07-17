//! ASR transcription **quality** test harness for the transcribe.cpp engine.
//!
//! Loading a model and getting *some* text back is not enough for a speech
//! model: an engine can be "wired up" yet transcribe the wrong language
//! (Chinese audio decoded as English / garbage) or return mush. This harness
//! feeds real speech and verifies the *content* of the transcript against a
//! known reference, so it catches those failures.
//!
//! Run:
//!
//!     cd src-tauri
//!     ASR_FIXTURES_DIR=/tmp cargo run --example tc_quality
//!
//! Prepare fixtures (models + audio) with `scripts/prepare-asr-fixtures.sh`.
//!
//! Environment:
//!   ASR_FIXTURES_DIR      directory holding the *.gguf models and *.wav audio
//!                         (default: /tmp)
//!   ASR_QUALITY_THRESHOLD minimum content coverage to PASS (default: 0.70)
//!
//! Scoring (deterministic, see functions below):
//!   1. norm()   — strip whitespace + punctuation, lowercase ASCII → pure content
//!   2. detect() — CJK vs ASCII-letter ratio → Zh / En / Unknown
//!   3. lang_ok  — detect(actual) must equal the case language; a mismatch is an
//!                 automatic FAIL even when content overlaps (the "wrong language"
//!                 guard)
//!   4. cov      — Zh: char-level LCS ratio; En: word-level coverage
//!   5. PASS iff  lang_ok && cov >= THRESHOLD
//!
//! Exit code: 1 if any case FAILs, 0 otherwise (SKIP does not fail).
//!
//! Scope: this harness is for **ASR / speech-to-text** GGUF models only. It
//! sends audio and checks recognized content. Non-ASR models — e.g. the
//! punctuation-restoration ("punct") model pulled in via transcribe-rs, or any
//! text-only post-processing model — do NOT take audio and MUST be tested
//! separately; do not add them here.

use std::collections::HashSet;
use std::path::Path;

use transcribe_cpp::{Model, RunOptions};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    Zh,
    En,
}

struct Case {
    label: &'static str,
    model_file: &'static str,
    audio_file: &'static str,
    expected: &'static str,
    lang: Lang,
}

/// The reference sentence spoken in `zh.wav` (see prepare-asr-fixtures.sh).
const ZH_TEXT: &str = "今天天气很好，我们一起去公园散步吧";
/// The reference sentence spoken in `jfk.wav` (the classic transcribe.cpp sample).
const EN_TEXT: &str = "And so my fellow Americans, ask not what your country can do for you, \
     ask what you can do for your country.";

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "moonshine-tiny-zh × zh.wav",
            model_file: "moonshine-tiny-zh.gguf",
            audio_file: "zh.wav",
            expected: ZH_TEXT,
            lang: Lang::Zh,
        },
        Case {
            label: "whisper-base × zh.wav",
            model_file: "whisper-base.gguf",
            audio_file: "zh.wav",
            expected: ZH_TEXT,
            lang: Lang::Zh,
        },
        Case {
            label: "whisper-base × jfk.wav",
            model_file: "whisper-base.gguf",
            audio_file: "jfk.wav",
            expected: EN_TEXT,
            lang: Lang::En,
        },
        Case {
            label: "sensevoice × zh.wav",
            model_file: "sensevoice.gguf",
            audio_file: "zh.wav",
            expected: ZH_TEXT,
            lang: Lang::Zh,
        },
        Case {
            label: "sensevoice × jfk.wav",
            model_file: "sensevoice.gguf",
            audio_file: "jfk.wav",
            expected: EN_TEXT,
            lang: Lang::En,
        },
    ]
}

// ---------------------------------------------------------------------------
// Scoring primitives
// ---------------------------------------------------------------------------

/// Is this char part of the "content" (a CJK ideograph, ASCII letter, or
/// digit)? Everything else — whitespace and every flavor of punctuation, ASCII
/// or full-width, quotes and brackets included — is *not* alphanumeric and gets
/// dropped by the normalizers.
fn is_content(c: char) -> bool {
    c.is_alphanumeric()
}

/// Normalize for Chinese scoring: keep only content chars (no spaces, no
/// punctuation), lowercase ASCII.
fn norm_zh(s: &str) -> String {
    s.chars()
        .filter(|c| is_content(*c))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Normalize for English scoring: drop punctuation and lowercase, but keep word
/// boundaries so we can split into words. Whitespace runs collapse to one space.
fn norm_en(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = true; // leading-space trim
    for c in s.chars() {
        if is_content(c) {
            out.push(c.to_ascii_lowercase());
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim_end().to_string()
}

/// Detect the dominant language of `s` by counting CJK ideographs against ASCII
/// letters (all other chars ignored). Returns `None` (Unknown) when there is no
/// signal or the split is exactly even.
fn detect(s: &str) -> Option<Lang> {
    let mut cjk = 0usize;
    let mut ascii = 0usize;
    for c in s.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            cjk += 1;
        } else if c.is_ascii_alphabetic() {
            ascii += 1;
        }
    }
    let total = cjk + ascii;
    if total == 0 {
        return None;
    }
    let cjk_ratio = cjk as f64 / total as f64;
    let ascii_ratio = ascii as f64 / total as f64;
    if cjk_ratio > 0.5 {
        Some(Lang::Zh)
    } else if ascii_ratio > 0.5 {
        Some(Lang::En)
    } else {
        None
    }
}

/// Length of the longest common subsequence of two char slices. Plain
/// O(n*m) DP — the transcripts here are short, so no third-party LCS needed.
fn lcs_len(a: &[char], b: &[char]) -> usize {
    let n = a.len();
    let m = b.len();
    if n == 0 || m == 0 {
        return 0;
    }
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }
    dp[n][m]
}

/// Content coverage of `actual` against `expected` in `[0.0, 1.0]`.
///
/// - Zh: char-level LCS ratio = LCS(expected, actual) / expected.chars().
/// - En: word-level coverage  = |expected words present in actual| / |expected words|.
fn coverage(expected: &str, actual: &str, lang: Lang) -> f64 {
    match lang {
        Lang::Zh => {
            let e: Vec<char> = norm_zh(expected).chars().collect();
            let a: Vec<char> = norm_zh(actual).chars().collect();
            if e.is_empty() {
                return 0.0;
            }
            lcs_len(&e, &a) as f64 / e.len() as f64
        }
        Lang::En => {
            let expected_words: Vec<String> = norm_en(expected)
                .split_whitespace()
                .map(str::to_string)
                .collect();
            if expected_words.is_empty() {
                return 0.0;
            }
            let actual_words: HashSet<String> = norm_en(actual)
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let hits = expected_words
                .iter()
                .filter(|w| actual_words.contains(*w))
                .count();
            hits as f64 / expected_words.len() as f64
        }
    }
}

// ---------------------------------------------------------------------------
// Audio + transcription
// ---------------------------------------------------------------------------

/// Decode a WAV file to 16 kHz mono f32 PCM (mirrors tc_spike.rs). The fixtures
/// are already 16 kHz mono; this only downmixes if a stray file has >1 channel.
fn load_wav_16k_mono(path: &Path) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<_, _>>()?,
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
    };
    let mono = if spec.channels > 1 {
        samples
            .chunks(spec.channels as usize)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    } else {
        samples
    };
    Ok(mono)
}

/// Transcribe one audio file with one model. No language hint is passed on
/// purpose — we want the model's own language decision so `detect()` can catch a
/// wrong-language failure.
fn transcribe(model_path: &Path, audio_path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let pcm = load_wav_16k_mono(audio_path)?;
    let model = Model::load(model_path)?;
    let caps = model.capabilities();
    let mut session = model.session()?;
    let result = session.run(
        &pcm,
        &RunOptions {
            timestamps: caps.max_timestamp_kind,
            ..Default::default()
        },
    )?;
    Ok(result.text.trim().to_string())
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

fn main() {
    let fixtures_dir = std::env::var("ASR_FIXTURES_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let threshold: f64 = std::env::var("ASR_QUALITY_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.70);

    println!(
        "ASR quality harness — transcribe.cpp {} ({})",
        transcribe_cpp::version(),
        transcribe_cpp::version_commit()
    );
    println!(
        "fixtures dir: {}  |  threshold: {:.2}\n",
        fixtures_dir, threshold
    );

    let dir = Path::new(&fixtures_dir);
    let (mut passed, mut failed, mut skipped) = (0usize, 0usize, 0usize);

    for case in cases() {
        let model_path = dir.join(case.model_file);
        let audio_path = dir.join(case.audio_file);

        if !model_path.exists() || !audio_path.exists() {
            println!("SKIP {}（缺 fixtures）", case.label);
            skipped += 1;
            continue;
        }

        match transcribe(&model_path, &audio_path) {
            Ok(actual) => {
                let lang_ok = detect(&actual) == Some(case.lang);
                let cov = coverage(case.expected, &actual, case.lang);
                let pass = lang_ok && cov >= threshold;
                println!(
                    "{} {} lang_ok={} cov={:.2} | 期望: {} | 实际: {}",
                    if pass { "PASS" } else { "FAIL" },
                    case.label,
                    lang_ok,
                    cov,
                    case.expected,
                    actual
                );
                if pass {
                    passed += 1;
                } else {
                    failed += 1;
                }
            }
            Err(e) => {
                println!("FAIL {} (转录错误: {})", case.label, e);
                failed += 1;
            }
        }
    }

    println!(
        "\n=== 汇总: {} passed / {} failed / {} skipped (threshold={:.2}) ===",
        passed, failed, skipped, threshold
    );

    if failed > 0 {
        std::process::exit(1);
    }
}
