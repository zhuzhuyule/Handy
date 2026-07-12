//! Phase 0 spike for the transcribe.cpp engine migration.
//!
//! Proves the crate loads a GGUF model and transcribes a WAV end-to-end in our
//! tree. Throwaway — delete once Phase 1 wires the engine into the app.
//!
//!     cargo run --example tc_spike -- /tmp/whisper-tiny-Q5_K_M.gguf /tmp/jfk.wav
//!
//! WAV is decoded with `hound` (already a project dep). Expects 16 kHz mono
//! 16-bit PCM (the jfk.wav sample is exactly that).

use transcribe_cpp::{Model, RunOptions};

fn load_wav_16k_mono(path: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<_, _>>()?,
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
    };
    // Downmix to mono if needed.
    let mono = if spec.channels > 1 {
        samples
            .chunks(spec.channels as usize)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    } else {
        samples
    };
    eprintln!(
        "wav: {} Hz, {} ch, {} samples ({:.1}s)",
        spec.sample_rate,
        spec.channels,
        mono.len(),
        mono.len() as f32 / spec.sample_rate as f32
    );
    Ok(mono)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model_path = args
        .next()
        .expect("usage: tc_spike <model.gguf> <audio.wav>");
    let audio_path = args
        .next()
        .expect("usage: tc_spike <model.gguf> <audio.wav>");

    println!(
        "transcribe-cpp {} ({})",
        transcribe_cpp::version(),
        transcribe_cpp::version_commit()
    );

    let pcm = load_wav_16k_mono(&audio_path)?;

    let model = Model::load(&model_path)?;
    let caps = model.capabilities();
    println!(
        "model arch={} backend={} max_ts={:?}",
        model.arch(),
        model.backend(),
        caps.max_timestamp_kind
    );

    let mut session = model.session()?;
    let result = session.run(
        &pcm,
        &RunOptions {
            timestamps: caps.max_timestamp_kind,
            ..Default::default()
        },
    )?;

    println!(
        "\nlanguage={} ts={:?} encode={:.0}ms",
        result.language.as_deref().unwrap_or("(n/a)"),
        result.timestamp_kind,
        result.timings.encode_ms
    );
    println!("\n=== TEXT ===\n{}\n", result.text.trim());
    Ok(())
}
