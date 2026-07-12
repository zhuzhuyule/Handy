//! Bundled ASR model catalog.
//!
//! The catalog JSON is vendored from upstream `cjpais/Handy` (tag `v0.9.1`)
//! and embedded at compile time. Every entry maps to a single-file GGUF model
//! hosted on the `handy-computer` Hugging Face org and downloaded through a
//! direct `resolve/main` URL (no `hf-hub` dependency).

use serde::Deserialize;

use crate::managers::model::{EngineType, ModelInfo};

/// Raw catalog JSON, embedded at compile time so the library never depends on
/// a runtime file being present.
const CATALOG_JSON: &str = include_str!("catalog.json");

/// Languages the model-library UI knows how to display/filter. Only these are
/// surfaced as tags so the language badges and filters stay meaningful even for
/// models that advertise dozens of languages.
const KNOWN_DISPLAY_LANGS: &[&str] = &["zh", "yue", "en", "ja", "ko", "de", "es", "fr", "ru"];

#[derive(Debug, Deserialize)]
struct CatalogRoot {
    #[serde(default)]
    models: Vec<CatalogModel>,
}

#[derive(Debug, Deserialize)]
struct CatalogModel {
    /// Hugging Face repo id, e.g. `handy-computer/whisper-medium-gguf`.
    #[serde(default)]
    id: String,
    /// Short, stable id used throughout the app (becomes `ModelInfo.id`).
    #[serde(default)]
    slug: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    family: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)] // parsed for completeness; not yet surfaced in the UI
    capabilities: CatalogCaps,
    /// 0-100 integer in the catalog; normalized to 0.0-1.0 on the way out.
    #[serde(default)]
    speed_score: f32,
    /// 0-100 integer in the catalog; normalized to 0.0-1.0 on the way out.
    #[serde(default)]
    accuracy_score: f32,
    #[serde(default)]
    files: Vec<CatalogFile>,
    #[serde(default)]
    default_quant: String,
    #[serde(default)]
    #[allow(dead_code)] // the recommended set is maintained on the frontend
    recommended: bool,
    #[serde(default)]
    #[allow(dead_code)]
    recommended_rank: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)] // parsed for tolerance; not consumed yet
struct CatalogCaps {
    #[serde(default)]
    streaming: bool,
    #[serde(default)]
    translate: bool,
    #[serde(default)]
    lang_detect: bool,
    /// `"none" | "token" | "segment" | "word"` in the catalog.
    #[serde(default)]
    timestamps: String,
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    filename: String,
    #[serde(default)]
    quant: String,
    #[serde(default)]
    size_bytes: u64,
}

/// Catalog scores are 0-100 integers; `ModelInfo` uses a 0.0-1.0 scale.
fn normalize_score(raw: f32) -> f32 {
    if raw > 1.0 {
        (raw / 100.0).clamp(0.0, 1.0)
    } else {
        raw.clamp(0.0, 1.0)
    }
}

/// Pick the download file: prefer the `default_quant` GGUF, otherwise the
/// smallest available GGUF.
fn pick_file(model: &CatalogModel) -> Option<&CatalogFile> {
    if !model.default_quant.is_empty() {
        if let Some(file) = model
            .files
            .iter()
            .find(|f| f.quant == model.default_quant && f.filename.ends_with(".gguf"))
        {
            return Some(file);
        }
    }

    model
        .files
        .iter()
        .filter(|f| f.filename.ends_with(".gguf"))
        .min_by_key(|f| f.size_bytes)
}

fn to_model_info(model: &CatalogModel) -> Option<ModelInfo> {
    if model.slug.is_empty() || model.id.is_empty() {
        return None;
    }

    let file = pick_file(model)?;

    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        model.id, file.filename
    );
    let size_mb = ((file.size_bytes as f64) / (1024.0 * 1024.0)).round() as u64;

    // Tags feed the library's grouping/filtering: family + known languages.
    let mut tags: Vec<String> = Vec::new();
    if !model.family.is_empty() {
        tags.push(model.family.clone());
    }
    for lang in &model.languages {
        if KNOWN_DISPLAY_LANGS.contains(&lang.as_str()) {
            tags.push(lang.clone());
        }
    }
    let tags = if tags.is_empty() { None } else { Some(tags) };

    let name = if model.name.is_empty() {
        model.slug.clone()
    } else {
        model.name.clone()
    };
    let description = if model.description.is_empty() {
        name.clone()
    } else {
        model.description.clone()
    };

    Some(ModelInfo {
        id: model.slug.clone(),
        name,
        description,
        filename: file.filename.clone(),
        url: Some(url),
        size_mb,
        is_downloaded: false,
        is_downloading: false,
        partial_size: 0,
        is_directory: false,
        engine_type: EngineType::TranscribeCpp,
        accuracy_score: normalize_score(model.accuracy_score),
        speed_score: normalize_score(model.speed_score),
        tags,
        // Every catalog entry is a built-in model (not user-added). The UI uses
        // this flag to hide the edit/remove actions on built-ins.
        is_default: true,
        sha256: None,
    })
}

/// Build the built-in model library from the embedded catalog.
pub fn catalog_models() -> Vec<ModelInfo> {
    match serde_json::from_str::<CatalogRoot>(CATALOG_JSON) {
        Ok(root) => root.models.iter().filter_map(to_model_info).collect(),
        Err(err) => {
            log::error!("Failed to parse bundled catalog.json: {err}");
            Vec::new()
        }
    }
}
