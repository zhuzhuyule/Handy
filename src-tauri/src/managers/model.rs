use crate::settings::{get_settings, write_settings};
use anyhow::Result;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use tar::Archive;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineType {
    /// Unified transcribe.cpp (ggml / GGUF) engine — the only local engine.
    TranscribeCpp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub filename: String,
    pub url: Option<String>,
    pub size_mb: u64,
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub partial_size: u64,
    pub is_directory: bool,
    pub engine_type: EngineType,
    pub accuracy_score: f32, // 0.0 to 1.0, higher is more accurate
    pub speed_score: f32,    // 0.0 to 1.0, higher is faster
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub is_default: bool, // True if it is a built-in default model
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserModelEntry {
    id: String,
    name: String,
    description: String,
    filename: String,
    url: String,
    size_mb: u64,
    is_directory: bool,
    #[serde(default = "default_engine_type")]
    engine_type: EngineType,
    accuracy_score: f32,
    speed_score: f32,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    sha256: Option<String>,
}

fn default_engine_type() -> EngineType {
    EngineType::TranscribeCpp
}

impl UserModelEntry {
    fn into_model_info(self) -> ModelInfo {
        ModelInfo {
            id: self.id,
            name: self.name,
            description: self.description,
            filename: self.filename,
            url: Some(self.url),
            size_mb: self.size_mb,
            is_downloaded: false,
            is_downloading: false,
            partial_size: 0,
            is_directory: self.is_directory,
            engine_type: self.engine_type,
            accuracy_score: self.accuracy_score,
            speed_score: self.speed_score,
            tags: self.tags,
            is_default: false,
            sha256: self.sha256,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

pub struct ModelManager {
    app_handle: AppHandle,
    models_dir: PathBuf,
    user_catalog_path: PathBuf,
    available_models: Mutex<HashMap<String, ModelInfo>>,
}

fn compute_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536]; // 64KB chunks
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let actual = compute_sha256(path)?;
    if actual != expected {
        // Delete the corrupt file
        let _ = fs::remove_file(path);
        return Err(anyhow::anyhow!(
            "SHA256 mismatch: expected {}, got {}. Corrupt file deleted.",
            expected,
            actual
        ));
    }
    Ok(())
}

impl ModelManager {
    fn read_user_catalog(path: &Path) -> Result<Vec<UserModelEntry>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(path)?;
        if data.trim().is_empty() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str::<Vec<UserModelEntry>>(&data)?)
    }

    fn write_user_catalog(path: &Path, entries: &[UserModelEntry]) -> Result<()> {
        let json = serde_json::to_string_pretty(entries)?;
        fs::write(path, json)?;
        Ok(())
    }

    fn calculate_dir_size(path: &Path) -> u64 {
        if path.is_file() {
            return path.metadata().map(|m| m.len()).unwrap_or(0);
        }
        fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| Self::calculate_dir_size(&e.path()))
                    .sum()
            })
            .unwrap_or(0)
    }

    fn update_user_catalog_size(&self, model_id: &str, size_mb: u64) -> Result<()> {
        let mut entries = Self::read_user_catalog(&self.user_catalog_path)?;
        if let Some(entry) = entries.iter_mut().find(|e| e.id == model_id) {
            entry.size_mb = size_mb;
            Self::write_user_catalog(&self.user_catalog_path, &entries)?;
        }
        Ok(())
    }

    fn filename_from_url(url: &str) -> Result<String> {
        let without_query = url.split('?').next().unwrap_or(url);
        let filename = without_query
            .split('/')
            .filter(|s| !s.is_empty())
            .last()
            .ok_or_else(|| anyhow::anyhow!("URL missing filename"))?;
        Ok(filename.to_string())
    }

    fn strip_archive_extensions(name: &str) -> String {
        let mut s = name.to_string();
        for ext in [".tar.bz2", ".tar.gz", ".tgz"] {
            if s.ends_with(ext) {
                s.truncate(s.len() - ext.len());
                return s;
            }
        }
        s
    }

    fn strip_known_prefixes(name: &str) -> String {
        let s = name.trim();
        let s = s.strip_prefix("sherpa-onnx-").unwrap_or(s);
        s.to_string()
    }

    fn infer_engine_type_from_name(_name: &str) -> EngineType {
        // Every local model now runs through the unified transcribe.cpp engine.
        EngineType::TranscribeCpp
    }

    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        // Create models directory in app data
        let models_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|e| anyhow::anyhow!("Failed to get app data dir: {}", e))?
            .join("models");

        if !models_dir.exists() {
            fs::create_dir_all(&models_dir)?;
        }

        let user_catalog_path = models_dir.join("catalog.user.json");

        let mut available_models = HashMap::new();

        // Populate the built-in library from the vendored catalog. Each entry
        // is a single-file GGUF model downloaded from a handy-computer Hugging
        // Face repo through a direct resolve URL.
        for model in crate::catalog::catalog_models() {
            available_models.insert(model.id.clone(), model);
        }

        // Punctuation (post-processing): zh+en mixed. This is NOT an ASR engine
        // — it is a standalone CT-Transformer model referenced by id through the
        // punct path (transcribe-rs `punct` feature). The `punct-` id prefix is
        // what the frontend uses to group it under "Punctuation". The
        // `engine_type` is irrelevant here (it never goes through `load_model`),
        // so we use the only variant that exists.
        available_models.insert(
            "punct-zh-en-ct-transformer-2024-04-12-int8".to_string(),
            ModelInfo {
                id: "punct-zh-en-ct-transformer-2024-04-12-int8".to_string(),
                name: "Punctuation Chinese + English".to_string(),
                description: "models.punct-zh-en-ct-transformer-2024-04-12-int8.description"
                    .to_string(),
                filename: "sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8"
                    .to_string(),
                url: Some("https://github.com/k2-fsa/sherpa-onnx/releases/download/punctuation-models/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8.tar.bz2".to_string()),
                size_mb: 62,
                is_downloaded: false,
                is_downloading: false,
                partial_size: 0,
                is_directory: true,
                engine_type: EngineType::TranscribeCpp,
                accuracy_score: 0.80,
                speed_score: 0.95,
                tags: None,
                is_default: true,
                sha256: None,
            },
        );

        // Merge user-provided catalog entries.
        if let Ok(user_entries) = Self::read_user_catalog(&user_catalog_path) {
            for entry in user_entries {
                let model = entry.into_model_info();
                available_models.entry(model.id.clone()).or_insert(model);
            }
        }

        let manager = Self {
            app_handle: app_handle.clone(),
            models_dir,
            user_catalog_path,
            available_models: Mutex::new(available_models),
        };

        // Migrate any bundled models to user directory
        manager.migrate_bundled_models()?;

        // Check which models are already downloaded
        manager.update_download_status()?;

        // Auto-select a model if none is currently selected
        manager.auto_select_model_if_needed()?;

        Ok(manager)
    }

    pub fn add_model_from_url(
        &self,
        url: String,
        name: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<String> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(anyhow::anyhow!("URL must start with http:// or https://"));
        }

        let archive_name = Self::filename_from_url(&url)?;
        let base_name = Self::strip_archive_extensions(&archive_name);
        let is_directory = archive_name.ends_with(".tar.gz")
            || archive_name.ends_with(".tgz")
            || archive_name.ends_with(".tar.bz2")
            || archive_name.ends_with(".bz2");
        let filename = if is_directory {
            base_name.clone()
        } else {
            archive_name.clone()
        };

        let preferred_id = Self::strip_known_prefixes(&base_name);

        // Check if it already exists
        {
            let models = self.available_models.lock().unwrap();

            if models.contains_key(&preferred_id) {
                // It exists. Check if it's a user model we can update.
                drop(models); // Release lock before file operations

                let mut entries = Self::read_user_catalog(&self.user_catalog_path)?;
                if let Some(pos) = entries.iter().position(|e| e.id == preferred_id) {
                    // Update existing user model
                    if let Some(n) = name {
                        entries[pos].name = n;
                    }
                    if let Some(t) = tags.clone() {
                        entries[pos].tags = Some(t);
                    }

                    Self::write_user_catalog(&self.user_catalog_path, &entries)?;

                    // Update in-memory
                    let mut models = self.available_models.lock().unwrap();
                    if let Some(m) = models.get_mut(&preferred_id) {
                        m.name = entries[pos].name.clone();
                        m.tags = entries[pos].tags.clone();
                    }
                    return Ok(preferred_id);
                } else {
                    // It's a built-in model, just return the ID
                    return Ok(preferred_id);
                }
            }
        }

        // Doesn't exist, proceed to creation
        let final_id = preferred_id; // Uniqueness guaranteed by check above

        let entry = UserModelEntry {
            id: final_id.clone(),
            name: name.unwrap_or_else(|| final_id.replace('-', " ")),
            description: "modelSelector.userAddedModel".to_string(),
            filename,
            url: url.clone(),
            size_mb: 0,
            is_directory,
            engine_type: Self::infer_engine_type_from_name(&base_name),
            accuracy_score: 0.8,
            speed_score: 0.8,
            tags,
            sha256: None,
        };

        let mut entries = Self::read_user_catalog(&self.user_catalog_path)?;
        entries.push(entry.clone());
        Self::write_user_catalog(&self.user_catalog_path, &entries)?;

        {
            let mut models = self.available_models.lock().unwrap();
            models.insert(final_id.clone(), entry.into_model_info());
        }
        self.update_download_status()?;

        Ok(final_id)
    }

    pub fn get_available_models(&self) -> Vec<ModelInfo> {
        let models = self.available_models.lock().unwrap();
        models.values().cloned().collect()
    }

    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        let models = self.available_models.lock().unwrap();
        models.get(model_id).cloned()
    }

    fn migrate_bundled_models(&self) -> Result<()> {
        // Check for bundled models and copy them to user directory
        let bundled_models = ["ggml-small.bin"]; // Add other bundled models here if any

        for filename in &bundled_models {
            let bundled_path = self.app_handle.path().resolve(
                &format!("resources/models/{}", filename),
                tauri::path::BaseDirectory::Resource,
            );

            if let Ok(bundled_path) = bundled_path {
                if bundled_path.exists() {
                    let user_path = self.models_dir.join(filename);

                    // Only copy if user doesn't already have the model
                    if !user_path.exists() {
                        info!("Migrating bundled model {} to user directory", filename);
                        fs::copy(&bundled_path, &user_path)?;
                        info!("Successfully migrated {}", filename);
                    }
                }
            }
        }

        Ok(())
    }

    fn update_download_status(&self) -> Result<()> {
        let mut size_updates: Vec<(String, u64)> = Vec::new();

        {
            let mut models = self.available_models.lock().unwrap();

            for model in models.values_mut() {
                if model.is_directory {
                    // For directory-based models, check if the directory exists
                    let model_path = self.models_dir.join(&model.filename);
                    let partial_path = self.models_dir.join(format!("{}.partial", &model.filename));
                    let extracting_path = self
                        .models_dir
                        .join(format!("{}.extracting", &model.filename));

                    // Clean up any leftover .extracting directories from interrupted extractions
                    if extracting_path.exists() {
                        warn!("Cleaning up interrupted extraction for model: {}", model.id);
                        let _ = fs::remove_dir_all(&extracting_path);
                    }

                    model.is_downloaded = model_path.exists() && model_path.is_dir();
                    model.is_downloading = false;

                    // Get partial file size if it exists (for the .tar.gz being downloaded)
                    if partial_path.exists() {
                        model.partial_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
                    } else {
                        model.partial_size = 0;
                    }

                    // Fix size_mb for downloaded models that have size_mb=0 (legacy data)
                    if model.is_downloaded && model.size_mb == 0 {
                        model.size_mb = Self::calculate_dir_size(&model_path) / (1024 * 1024);
                        size_updates.push((model.id.clone(), model.size_mb));
                    }
                } else {
                    // For file-based models (existing logic)
                    let model_path = self.models_dir.join(&model.filename);
                    let partial_path = self.models_dir.join(format!("{}.partial", &model.filename));

                    model.is_downloaded = model_path.exists();
                    model.is_downloading = false;

                    // Get partial file size if it exists
                    if partial_path.exists() {
                        model.partial_size = partial_path.metadata().map(|m| m.len()).unwrap_or(0);
                    } else {
                        model.partial_size = 0;
                    }

                    // Fix size_mb for downloaded models that have size_mb=0 (legacy data)
                    if model.is_downloaded && model.size_mb == 0 {
                        model.size_mb =
                            model_path.metadata().map(|m| m.len()).unwrap_or(0) / (1024 * 1024);
                        size_updates.push((model.id.clone(), model.size_mb));
                    }
                }
            }
        }

        // Persist size_mb fixes to user catalog for any updated models
        if !size_updates.is_empty() {
            if let Ok(mut entries) = Self::read_user_catalog(&self.user_catalog_path) {
                let mut changed = false;
                for (id, size_mb) in &size_updates {
                    if let Some(entry) = entries.iter_mut().find(|e| e.id == *id) {
                        entry.size_mb = *size_mb;
                        changed = true;
                    }
                }
                if changed {
                    let _ = Self::write_user_catalog(&self.user_catalog_path, &entries);
                }
            }
        }

        Ok(())
    }

    fn auto_select_model_if_needed(&self) -> Result<()> {
        let settings = get_settings(&self.app_handle);

        // The selection is valid only if it still exists in the catalog. After
        // the transcribe.cpp migration, legacy ids (e.g. "small", "parakeet-*")
        // no longer exist, so fall back to a default rather than leaving a
        // dangling selection that would fail to load.
        let pick = {
            let models = self.available_models.lock().unwrap();
            let selection_valid = !settings.selected_model.is_empty()
                && models.contains_key(&settings.selected_model);
            if selection_valid {
                None
            } else {
                // Prefer a downloaded model, then the default whisper-tiny, then
                // any known model.
                models
                    .values()
                    .find(|model| model.is_downloaded)
                    .map(|m| m.id.clone())
                    .or_else(|| {
                        if models.contains_key("whisper-tiny") {
                            Some("whisper-tiny".to_string())
                        } else {
                            models.keys().next().cloned()
                        }
                    })
            }
        };

        if let Some(model_id) = pick {
            info!("Auto-selecting model: {}", model_id);
            let mut updated_settings = settings;
            updated_settings.selected_model = model_id.clone();
            write_settings(&self.app_handle, updated_settings);
            info!("Successfully auto-selected model: {}", model_id);
        }

        Ok(())
    }

    pub async fn download_model(&self, model_id: &str) -> Result<()> {
        let model_info = {
            let models = self.available_models.lock().unwrap();
            models.get(model_id).cloned()
        };

        let model_info =
            model_info.ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        let url = model_info
            .url
            .ok_or_else(|| anyhow::anyhow!("No download URL for model"))?;
        let model_path = self.models_dir.join(&model_info.filename);
        let partial_path = self
            .models_dir
            .join(format!("{}.partial", &model_info.filename));

        // Don't download if complete version already exists
        if model_path.exists() {
            // Clean up any partial file that might exist
            if partial_path.exists() {
                let _ = fs::remove_file(&partial_path);
            }
            self.update_download_status()?;
            return Ok(());
        }

        // Check if we have a partial download to resume
        let mut resume_from = if partial_path.exists() {
            let size = partial_path.metadata()?.len();
            info!("Resuming download of model {} from byte {}", model_id, size);
            size
        } else {
            info!("Starting fresh download of model {} from {}", model_id, url);
            0
        };

        // Mark as downloading
        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = true;
            }
        }

        // Create HTTP client with range request for resuming
        let client = crate::http_client::build_http_client(
            None,
            std::time::Duration::from_secs(300),
            reqwest::header::HeaderMap::new(),
        )
        .map_err(|e| anyhow::anyhow!(e))?;
        let mut request = client.get(&url);

        if resume_from > 0 {
            request = request.header("Range", format!("bytes={}-", resume_from));
        }

        let mut response = request.send().await?;

        // If we tried to resume but server returned 200 (not 206 Partial Content),
        // the server doesn't support range requests. Delete partial file and restart
        // fresh to avoid file corruption (appending full file to partial).
        if resume_from > 0 && response.status() == reqwest::StatusCode::OK {
            warn!(
                "Server doesn't support range requests for model {}, restarting download",
                model_id
            );
            drop(response);
            let _ = fs::remove_file(&partial_path);

            // Reset resume_from since we're starting fresh
            resume_from = 0;

            // Restart download without range header
            response = client.get(&url).send().await?;
        }

        // Check for success or partial content status
        if !response.status().is_success()
            && response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        {
            // Mark as not downloading on error
            {
                let mut models = self.available_models.lock().unwrap();
                if let Some(model) = models.get_mut(model_id) {
                    model.is_downloading = false;
                }
            }
            return Err(anyhow::anyhow!(
                "Failed to download model: HTTP {}",
                response.status()
            ));
        }

        let total_size = if resume_from > 0 {
            // For resumed downloads, add the resume point to content length
            resume_from + response.content_length().unwrap_or(0)
        } else {
            response.content_length().unwrap_or(0)
        };

        let mut downloaded = resume_from;
        let mut stream = response.bytes_stream();

        // Open file for appending if resuming, or create new if starting fresh
        let mut file = if resume_from > 0 {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&partial_path)?
        } else {
            std::fs::File::create(&partial_path)?
        };

        // Emit initial progress
        let initial_progress = DownloadProgress {
            model_id: model_id.to_string(),
            downloaded,
            total: total_size,
            percentage: if total_size > 0 {
                (downloaded as f64 / total_size as f64) * 100.0
            } else {
                0.0
            },
        };
        let _ = self
            .app_handle
            .emit("model-download-progress", &initial_progress);

        // Download with progress
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                // Mark as not downloading on error
                {
                    let mut models = self.available_models.lock().unwrap();
                    if let Some(model) = models.get_mut(model_id) {
                        model.is_downloading = false;
                    }
                }
                e
            })?;

            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;

            let percentage = if total_size > 0 {
                (downloaded as f64 / total_size as f64) * 100.0
            } else {
                0.0
            };

            // Emit progress event
            let progress = DownloadProgress {
                model_id: model_id.to_string(),
                downloaded,
                total: total_size,
                percentage,
            };

            let _ = self.app_handle.emit("model-download-progress", &progress);
        }

        file.flush()?;
        drop(file); // Ensure file is closed before moving

        // Verify downloaded file size matches expected size
        if total_size > 0 {
            let actual_size = partial_path.metadata()?.len();
            if actual_size != total_size {
                // Download is incomplete/corrupted - delete partial and return error
                let _ = fs::remove_file(&partial_path);
                {
                    let mut models = self.available_models.lock().unwrap();
                    if let Some(model) = models.get_mut(model_id) {
                        model.is_downloading = false;
                    }
                }
                return Err(anyhow::anyhow!(
                    "Download incomplete: expected {} bytes, got {} bytes",
                    total_size,
                    actual_size
                ));
            }
        }

        // Verify SHA256 if hash is available
        if let Some(ref expected_hash) = model_info.sha256 {
            info!("Verifying SHA256 for model: {}", model_id);
            if let Err(e) = verify_sha256(&partial_path, expected_hash) {
                let mut models = self.available_models.lock().unwrap();
                if let Some(model) = models.get_mut(model_id) {
                    model.is_downloading = false;
                    model.partial_size = 0;
                }
                let _ = self.app_handle.emit(
                    "model-download-error",
                    &serde_json::json!({
                        "model_id": model_id,
                        "error": format!("{}", e)
                    }),
                );
                return Err(e);
            }
            info!("SHA256 verified for model: {}", model_id);
        }

        // Handle directory-based models (extract tar.gz) vs file-based models
        if model_info.is_directory {
            // Emit extraction started event
            let _ = self.app_handle.emit("model-extraction-started", model_id);
            info!("Extracting archive for directory-based model: {}", model_id);

            // Use a temporary extraction directory to ensure atomic operations
            let temp_extract_dir = self
                .models_dir
                .join(format!("{}.extracting", &model_info.filename));
            let final_model_dir = self.models_dir.join(&model_info.filename);

            // Clean up any previous incomplete extraction
            if temp_extract_dir.exists() {
                let _ = fs::remove_dir_all(&temp_extract_dir);
            }

            // Create temporary extraction directory
            fs::create_dir_all(&temp_extract_dir)?;

            // Open the downloaded archive file
            let archive_file = File::open(&partial_path)?;
            let url_path = url.split('?').next().unwrap_or(&url);
            let reader: Box<dyn Read> = if url_path.ends_with(".bz2") {
                Box::new(bzip2::read::BzDecoder::new(archive_file))
            } else {
                // Default to gzip
                Box::new(GzDecoder::new(archive_file))
            };
            let mut archive = Archive::new(reader);

            // Extract to the temporary directory first
            archive.unpack(&temp_extract_dir).map_err(|e| {
                let error_msg = format!("Failed to extract archive: {}", e);
                // Clean up failed extraction
                let _ = fs::remove_dir_all(&temp_extract_dir);
                let _ = self.app_handle.emit(
                    "model-extraction-failed",
                    &serde_json::json!({
                        "model_id": model_id,
                        "error": error_msg
                    }),
                );
                anyhow::anyhow!(error_msg)
            })?;

            // Find the actual extracted directory (archive might have a nested structure)
            let extracted_dirs: Vec<_> = fs::read_dir(&temp_extract_dir)?
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .collect();

            if extracted_dirs.len() == 1 {
                // Single directory extracted, move it to the final location
                let source_dir = extracted_dirs[0].path();
                if final_model_dir.exists() {
                    fs::remove_dir_all(&final_model_dir)?;
                }
                fs::rename(&source_dir, &final_model_dir)?;
                // Clean up temp directory
                let _ = fs::remove_dir_all(&temp_extract_dir);
            } else {
                // Multiple items or no directories, rename the temp directory itself
                if final_model_dir.exists() {
                    fs::remove_dir_all(&final_model_dir)?;
                }
                fs::rename(&temp_extract_dir, &final_model_dir)?;
            }

            info!("Successfully extracted archive for model: {}", model_id);
            // Emit extraction completed event
            let _ = self.app_handle.emit("model-extraction-completed", model_id);

            // Remove the downloaded tar.gz file
            let _ = fs::remove_file(&partial_path);
        } else {
            // Move partial file to final location for file-based models
            fs::rename(&partial_path, &model_path)?;
        }

        // Calculate actual file size after download
        let actual_size_mb = if model_info.is_directory {
            Self::calculate_dir_size(&model_path) / (1024 * 1024)
        } else {
            model_path.metadata().map(|m| m.len()).unwrap_or(0) / (1024 * 1024)
        };

        // Update download status and size in memory
        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = false;
                model.is_downloaded = true;
                model.partial_size = 0;
                model.size_mb = actual_size_mb;
            }
        }

        // Persist size_mb to user catalog
        let _ = self.update_user_catalog_size(model_id, actual_size_mb);

        // Emit completion event
        let _ = self.app_handle.emit("model-download-complete", model_id);

        info!(
            "Successfully downloaded model {} to {:?}",
            model_id, model_path
        );

        Ok(())
    }

    pub fn delete_model(&self, model_id: &str) -> Result<()> {
        debug!("ModelManager: delete_model called for: {}", model_id);

        let model_info = {
            let models = self.available_models.lock().unwrap();
            models.get(model_id).cloned()
        };

        let model_info =
            model_info.ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        debug!("ModelManager: Found model info: {:?}", model_info);

        let model_path = self.models_dir.join(&model_info.filename);
        let partial_path = self
            .models_dir
            .join(format!("{}.partial", &model_info.filename));
        debug!("ModelManager: Model path: {:?}", model_path);
        debug!("ModelManager: Partial path: {:?}", partial_path);

        let mut deleted_something = false;

        if model_info.is_directory {
            // Delete complete model directory if it exists
            if model_path.exists() && model_path.is_dir() {
                info!("Deleting model directory at: {:?}", model_path);
                fs::remove_dir_all(&model_path)?;
                info!("Model directory deleted successfully");
                deleted_something = true;
            }
        } else {
            // Delete complete model file if it exists
            if model_path.exists() {
                info!("Deleting model file at: {:?}", model_path);
                fs::remove_file(&model_path)?;
                info!("Model file deleted successfully");
                deleted_something = true;
            }
        }

        // Delete partial file if it exists (same for both types)
        if partial_path.exists() {
            info!("Deleting partial file at: {:?}", partial_path);
            fs::remove_file(&partial_path)?;
            info!("Partial file deleted successfully");
            deleted_something = true;
        }

        if !deleted_something {
            return Err(anyhow::anyhow!("No model files found to delete"));
        }

        // Update download status
        self.update_download_status()?;
        debug!("ModelManager: download status updated");

        Ok(())
    }

    pub fn get_model_path(&self, model_id: &str) -> Result<PathBuf> {
        let model_info = self
            .get_model_info(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        if !model_info.is_downloaded {
            return Err(anyhow::anyhow!("Model not available: {}", model_id));
        }

        // Ensure we don't return partial files/directories
        if model_info.is_downloading {
            return Err(anyhow::anyhow!(
                "Model is currently downloading: {}",
                model_id
            ));
        }

        let model_path = self.models_dir.join(&model_info.filename);
        let partial_path = self
            .models_dir
            .join(format!("{}.partial", &model_info.filename));

        if model_info.is_directory {
            // For directory-based models, ensure the directory exists and is complete
            if model_path.exists() && model_path.is_dir() && !partial_path.exists() {
                Ok(model_path)
            } else {
                Err(anyhow::anyhow!(
                    "Complete model directory not found: {}",
                    model_id
                ))
            }
        } else {
            // For file-based models (existing logic)
            if model_path.exists() && !partial_path.exists() {
                Ok(model_path)
            } else {
                Err(anyhow::anyhow!(
                    "Complete model file not found: {}",
                    model_id
                ))
            }
        }
    }

    pub fn cancel_download(&self, model_id: &str) -> Result<()> {
        debug!("ModelManager: cancel_download called for: {}", model_id);

        let _model_info = {
            let models = self.available_models.lock().unwrap();
            models.get(model_id).cloned()
        };

        let _model_info =
            _model_info.ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

        // Mark as not downloading
        {
            let mut models = self.available_models.lock().unwrap();
            if let Some(model) = models.get_mut(model_id) {
                model.is_downloading = false;
            }
        }

        // Note: The actual download cancellation would need to be handled
        // by the download task itself. This just updates the state.
        // The partial file is kept so the download can be resumed later.

        // Update download status to reflect current state
        self.update_download_status()?;

        info!("Download cancelled for: {}", model_id);
        Ok(())
    }

    pub fn remove_custom_model(&self, model_id: &str, delete_files: bool) -> Result<()> {
        info!("ModelManager: remove_custom_model called for: {}", model_id);

        // 1. Remove from user catalog
        let mut entries = Self::read_user_catalog(&self.user_catalog_path)?;
        let initial_len = entries.len();
        entries.retain(|e| e.id != model_id);

        if entries.len() == initial_len {
            return Err(anyhow::anyhow!(
                "Custom model not found in catalog: {}",
                model_id
            ));
        }

        Self::write_user_catalog(&self.user_catalog_path, &entries)?;

        // 2. Remove files if requested
        if delete_files {
            // We use the existing logic but ignore error if files don't exist
            let _ = self.delete_model(model_id);
        }

        // 3. Remove from in-memory map
        {
            let mut models = self.available_models.lock().unwrap();
            models.remove(model_id);
        }

        // 4. Update download status (triggers refresh in frontend usually)
        self.update_download_status()?;

        Ok(())
    }
}
