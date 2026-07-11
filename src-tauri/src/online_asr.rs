use crate::settings::PostProcessProvider;
use anyhow::{anyhow, Context, Result};
use reqwest::blocking::multipart;
use serde::Serialize;
use std::io::Cursor;
use std::time::Duration;

const DEFAULT_TRANSCRIPTION_PATH: &str = "audio/transcriptions";

#[allow(dead_code)]
#[derive(Clone, Serialize)]
pub struct OnlineAsrStatusEvent {
    pub stage: String,
    pub detail: Option<String>,
}

pub struct OnlineAsrClient {
    pub sample_rate: u32,
    pub timeout: Duration,
}

impl OnlineAsrClient {
    pub fn new(sample_rate: u32, timeout: Duration) -> Self {
        Self {
            sample_rate,
            timeout,
        }
    }

    pub fn transcribe(
        &self,
        provider: &PostProcessProvider,
        api_key: Option<String>,
        model_id: &str,
        language: Option<&str>,
        samples: &[f32],
    ) -> Result<String> {
        let wav_bytes = encode_wav(samples, self.sample_rate)?;
        self.transcribe_audio_bytes(
            provider,
            api_key,
            model_id,
            language,
            &wav_bytes,
            Some("recording.wav"),
            Some("audio/wav"),
        )
    }

    pub fn transcribe_audio_bytes(
        &self,
        provider: &PostProcessProvider,
        api_key: Option<String>,
        model_id: &str,
        language: Option<&str>,
        audio_bytes: &[u8],
        file_name: Option<&str>,
        mime_type: Option<&str>,
    ) -> Result<String> {
        let url = format!(
            "{}/{}",
            provider.base_url.trim_end_matches('/'),
            DEFAULT_TRANSCRIPTION_PATH
        );

        log::info!(
            "Online ASR request: url={}, model={}, language={:?}, audio_bytes={}, use_proxy={}",
            url,
            model_id,
            language,
            audio_bytes.len(),
            provider.use_proxy
        );
        // Per-provider proxy policy: providers with use_proxy=false connect
        // directly (no system-proxy inheritance), so a domestic endpoint is not
        // dragged through a proxy the user set only for overseas providers.
        let client = crate::http_client::build_blocking_http_client_with_policy(
            None,
            provider.use_proxy,
            self.timeout,
        )
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to build HTTP client")?;

        let mut form = multipart::Form::new()
            .part(
                "file",
                multipart::Part::bytes(audio_bytes.to_vec())
                    .file_name(file_name.unwrap_or("recording.wav").to_string())
                    .mime_str(mime_type.unwrap_or("audio/wav"))?,
            )
            .text("model", model_id.to_string());

        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }

        let mut request = client.post(&url).multipart(form);
        if let Some(key) = api_key {
            request = request.bearer_auth(key.trim());
        }

        let req_start = std::time::Instant::now();
        let response = request.send().with_context(|| {
            format!(
                "failed to send transcription request to {} after {}ms",
                url,
                req_start.elapsed().as_millis()
            )
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .unwrap_or_else(|_| "Failed to read body".to_string());
            log::warn!(
                "Online ASR failed: model={}, status={}, elapsed={}ms",
                model_id,
                status,
                req_start.elapsed().as_millis()
            );
            return Err(anyhow!(
                "remote transcription failed ({}): {}",
                status,
                text
            ));
        }
        log::info!(
            "Online ASR response: model={}, status={}, elapsed={}ms",
            model_id,
            response.status(),
            req_start.elapsed().as_millis()
        );

        let body: serde_json::Value = response
            .json()
            .context("failed to deserialize transcription response")?;

        let text = body
            .get("text")
            .or_else(|| body.get("result"))
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("transcription missing text field: {:?}", body))?
            .to_string();

        Ok(text)
    }
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer =
            hound::WavWriter::new(&mut cursor, spec).context("failed to create WAV writer")?;
        for &sample in samples.iter() {
            let clamped = (sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32);
            writer
                .write_sample(clamped as i16)
                .context("failed to write sample")?;
        }
        writer.finalize().context("failed to finalize WAV")?;
    }

    Ok(cursor.into_inner())
}
