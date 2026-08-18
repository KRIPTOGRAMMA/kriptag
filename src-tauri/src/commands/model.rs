use std::io::Write;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

// The recommended default model: a small instruction-tuned GGUF that runs on a
// CPU. The URL field in the UI is editable, so any other GGUF can be used.
pub const DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf";

/// Which engine a model feeds. The app downloads two unrelated kinds of model,
/// and the only thing that actually differs between them is the file they land
/// in — the catalogue, the progress reporting and the download itself are shared.
///
/// The file name is fixed per kind rather than derived from the URL: the picker
/// lets the user paste an arbitrary URL, and sidecar.rs looks the file up by a
/// known name. A model swapped for another of the same kind therefore replaces
/// the previous one instead of piling up unreferenced files.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    /// The chat model behind every AI feature (llamafile sidecar).
    Llm,
    /// Speech recognition for voice input (whisper.cpp sidecar).
    Whisper,
}

impl ModelKind {
    /// The on-disk name inside the models directory.
    pub fn file_name(self) -> &'static str {
        match self {
            // Kept as-is: it is what sidecar.rs already loads and what existing
            // installations have on disk. Renaming it would silently orphan a
            // model the user has already downloaded.
            ModelKind::Llm => "model.gguf",
            ModelKind::Whisper => "whisper.bin",
        }
    }

    /// Where the url of the installed model is remembered.
    ///
    /// Needed because file_name() above is deliberately fixed: the file says a
    /// model exists but not which one, and the picker has to show the one that
    /// is actually installed rather than the recommended one.
    pub fn installed_setting_key(self) -> &'static str {
        match self {
            ModelKind::Llm => "installed_model_url",
            ModelKind::Whisper => "installed_whisper_url",
        }
    }
}

#[derive(Clone, Serialize)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub url: String,
    pub size_bytes: u64,
    pub description: String,
    pub ram_gb: u8,
    pub recommended: bool,
    pub kind: ModelKind,
}

// A curated list of real GGUF quantizations from HuggingFace, verified by hand
// (no invented URLs or sizes). size_bytes is the size of the quantization file on
// HF at the time it was added to the list; it may differ slightly from the
// current one if the author re-uploaded the file, which is not critical — it is
// a hint for the user before downloading, not an integrity check.
// download_model() and model_status() are unaffected: any model from the list (or
// a manually pasted URL, since the field stays editable) is stored under the same
// model.gguf name — sidecar.rs is not tied to a particular model.
pub fn model_catalog() -> Vec<ModelOption> {
    vec![
        ModelOption {
            kind: ModelKind::Llm,
            id: "qwen2.5-0.5b".into(),
            name: "Qwen2.5 0.5B Instruct".into(),
            url: DEFAULT_MODEL_URL.into(),
            size_bytes: 491_000_000,
            description: "Самая быстрая и лёгкая — годится для слабых машин и старых ноутбуков, но качество ответов базовое.".into(),
            ram_gb: 2,
            recommended: false,
        },
        ModelOption {
            kind: ModelKind::Llm,
            id: "qwen2.5-1.5b".into(),
            name: "Qwen2.5 1.5B Instruct".into(),
            url: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf".into(),
            size_bytes: 1_120_000_000,
            description: "Баланс скорости и качества — заметно лучше 0.5B в рассуждениях, всё ещё быстрая на CPU.".into(),
            ram_gb: 3,
            recommended: true,
        },
        ModelOption {
            kind: ModelKind::Llm,
            id: "phi-3.5-mini".into(),
            name: "Phi-3.5 Mini Instruct".into(),
            url: "https://huggingface.co/bartowski/Phi-3.5-mini-instruct-GGUF/resolve/main/Phi-3.5-mini-instruct-Q4_K_M.gguf".into(),
            size_bytes: 2_390_000_000,
            description: "Лучшее качество из трёх — точнее держит инструкции и контекст, но медленнее и требует больше памяти.".into(),
            ram_gb: 5,
            recommended: false,
        },
    ]
}

// Speech recognition models for voice input, from the whisper.cpp author's own
// HuggingFace repository — the canonical source for the ggml format, with stable
// file names.
//
// Only multilingual builds are listed: the .en variants are smaller and more
// accurate but understand English alone, and the interface already ships in two
// languages. The sizes are those of the ggml files at the time they were added,
// used as a hint before downloading rather than as an integrity check — the same
// convention as model_catalog().
//
// ram_gb here means the memory whisper.cpp needs while transcribing, not the size
// of the download; base is the recommendation because tiny drops noticeably more
// words on Russian, which is what the author of this app dictates in.
pub fn whisper_catalog() -> Vec<ModelOption> {
    vec![
        ModelOption {
            kind: ModelKind::Whisper,
            id: "whisper-tiny".into(),
            name: "Whisper Tiny".into(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin".into(),
            size_bytes: 77_700_000,
            description: "Самая лёгкая и быстрая — распознаёт почти мгновенно даже на слабой машине, но заметно путает слова, особенно в русской речи.".into(),
            ram_gb: 1,
            recommended: false,
        },
        ModelOption {
            kind: ModelKind::Whisper,
            id: "whisper-base".into(),
            name: "Whisper Base".into(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin".into(),
            size_bytes: 148_000_000,
            description: "Разумный баланс — держит русскую речь заметно лучше Tiny и всё ещё быстрая на обычном процессоре.".into(),
            ram_gb: 1,
            recommended: true,
        },
        ModelOption {
            kind: ModelKind::Whisper,
            id: "whisper-small".into(),
            name: "Whisper Small".into(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin".into(),
            size_bytes: 488_000_000,
            description: "Самая точная из трёх — уверенно разбирает длинные фразы и имена, но расшифровка занимает в разы больше времени.".into(),
            ram_gb: 2,
            recommended: false,
        },
    ]
}

/// The catalogue for one kind. Defaults to the chat models so the existing
/// callers keep working unchanged.
#[tauri::command]
pub fn list_model_options(kind: Option<ModelKind>) -> Vec<ModelOption> {
    match kind.unwrap_or(ModelKind::Llm) {
        ModelKind::Llm => model_catalog(),
        ModelKind::Whisper => whisper_catalog(),
    }
}

#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    pub pct: u8,
    // Both kinds report through the same event, so the listener has to be able to
    // tell whose progress this is: without it the whisper picker would animate
    // while the chat model downloads, and vice versa.
    pub kind: ModelKind,
}

#[derive(Serialize)]
pub struct ModelStatus {
    pub exists: bool,
    pub size_bytes: u64,
    /// The url of the model that is actually installed, or None when nothing was
    /// downloaded through this app. `exists` alone cannot answer it: the file has
    /// a fixed name and carries no identity.
    pub installed_url: Option<String>,
}

// Also the working directory whisper is run from, so that the model and the
// recording can both be named relatively — see voice/mod.rs.
pub(crate) fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

// Whether a local model of that kind has been downloaded. Creates no directory —
// a check only.
pub(crate) fn model_available(app: &AppHandle, kind: ModelKind) -> bool {
    app.path()
        .app_data_dir()
        .map(|d| d.join("models").join(kind.file_name()).exists())
        .unwrap_or(false)
}

// Whether the chat model is present. Kept as a named wrapper because the AI
// capability check reads better this way and it is the overwhelmingly common case.
pub(crate) fn local_model_available(app: &AppHandle) -> bool {
    model_available(app, ModelKind::Llm)
}

// The path to the model file, shown in Settings. It used to be hardcoded in the
// UI as `~/.local/share/kriptag/models/model.gguf`, which was wrong on
// Windows/macOS and wrong even on Linux (the directory is named after the
// identifier, com.kriptag.app, not kriptag). We return what models_dir actually
// uses instead of assembling a per-OS string in the frontend: a platform
// substitution there would drift from the backend at the first change.
#[tauri::command]
pub async fn model_path(app: AppHandle, kind: Option<ModelKind>) -> Result<String, String> {
    let kind = kind.unwrap_or(ModelKind::Llm);
    Ok(models_dir(&app)?.join(kind.file_name()).display().to_string())
}

#[tauri::command]
pub async fn model_status(app: AppHandle, kind: Option<ModelKind>) -> Result<ModelStatus, String> {
    let kind = kind.unwrap_or(ModelKind::Llm);
    let path = models_dir(&app)?.join(kind.file_name());
    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let exists = path.exists();

    // Only report the remembered url while the file is really there: deleting the
    // model outside the app would otherwise leave the picker claiming an install
    // that no longer exists.
    let installed_url = if exists {
        match app.try_state::<sqlx::SqlitePool>() {
            Some(pool) => {
                crate::commands::settings::get_setting(pool.inner(), kind.installed_setting_key()).await
            }
            None => None,
        }
    } else {
        None
    };

    Ok(ModelStatus { exists, size_bytes, installed_url })
}

#[tauri::command]
pub async fn download_model(app: AppHandle, url: String, kind: Option<ModelKind>) -> Result<(), String> {
    let kind = kind.unwrap_or(ModelKind::Llm);
    let dir = models_dir(&app)?;
    let final_path = dir.join(kind.file_name());
    // Download into .part and rename only once complete, so an interrupted
    // download does not look like a ready model.
    let part_path = dir.join(format!("{}.part", kind.file_name()));

    let mut resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Сервер вернул {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(&part_path).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut last_pct: u8 = 255; // deliberately impossible so the first emit gets through

    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if let Err(e) = file.write_all(&chunk) {
            let _ = std::fs::remove_file(&part_path);
            return Err(e.to_string());
        }
        downloaded += chunk.len() as u64;
        let pct = if total > 0 { ((downloaded * 100) / total).min(100) as u8 } else { 0 };
        if pct != last_pct {
            last_pct = pct;
            let _ = app.emit("model-download-progress", DownloadProgress { downloaded, total, pct, kind });
        }
    }

    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&part_path, &final_path).map_err(|e| e.to_string())?;

    // Remember WHICH model this is. The file is always saved under the same name
    // (model.gguf), so the filesystem cannot answer the question afterwards, and
    // without an answer the picker had nothing to restore: it fell back to the
    // recommended entry every time Settings was reopened, silently misreporting
    // what is actually installed.
    //
    // Written after the rename, not before: only a finished download is a model
    // the user has. The url is stored rather than the catalogue id, because a
    // custom url has no id at all, and matching by url covers both cases.
    if let Some(pool) = app.try_state::<sqlx::SqlitePool>() {
        let _ = crate::commands::settings::set_setting(
            pool.inner(), kind.installed_setting_key(), &url,
        ).await;
    }

    let _ = app.emit("model-download-progress", DownloadProgress { downloaded, total, pct: 100, kind });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // Every invariant below now holds for both catalogues: they are downloaded by
    // the same code and shown by the same component, so a rule that mattered for
    // one of them matters for the other.
    fn catalogs() -> [(&'static str, Vec<ModelOption>); 2] {
        [("llm", model_catalog()), ("whisper", whisper_catalog())]
    }

    #[test]
    fn catalog_ids_are_unique_and_non_empty() {
        for (name, catalog) in catalogs() {
            assert!(!catalog.is_empty(), "{name}: пустой каталог");
            let ids: HashSet<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
            assert_eq!(ids.len(), catalog.len(), "{name}: повторяющиеся id");
        }
    }

    #[test]
    fn catalog_has_exactly_one_recommended_entry() {
        for (name, catalog) in catalogs() {
            let recommended = catalog.into_iter().filter(|m| m.recommended).count();
            assert_eq!(recommended, 1, "{name}: рекомендованных должно быть ровно одна");
        }
    }

    #[test]
    fn catalog_contains_default_model_url_entry() {
        let catalog = model_catalog();
        assert!(catalog.iter().any(|m| m.url == DEFAULT_MODEL_URL));
    }

    #[test]
    fn catalog_urls_are_https_and_unique() {
        for (name, catalog) in catalogs() {
            let urls: HashSet<&str> = catalog.iter().map(|m| m.url.as_str()).collect();
            assert_eq!(urls.len(), catalog.len(), "{name}: повторяющиеся URL");
            assert!(catalog.iter().all(|m| m.url.starts_with("https://")), "{name}: не-https URL");
        }
    }

    // Every entry must carry its own kind: the frontend picks the file to write and
    // the progress to listen to from this field, so a mislabelled entry would
    // download a whisper model over the chat one.
    #[test]
    fn every_entry_carries_its_own_kind() {
        assert!(model_catalog().iter().all(|m| m.kind == ModelKind::Llm));
        assert!(whisper_catalog().iter().all(|m| m.kind == ModelKind::Whisper));
    }

    // The two kinds must never share a file name, or downloading one would destroy
    // the other.
    #[test]
    fn kinds_have_distinct_file_names() {
        assert_ne!(ModelKind::Llm.file_name(), ModelKind::Whisper.file_name());
    }

    // The chat model's file name is load-bearing: sidecar.rs opens it by this exact
    // name and existing installations already have it on disk. Renaming it would
    // silently orphan a model the user has already downloaded.
    #[test]
    fn llm_file_name_stays_the_historical_one() {
        assert_eq!(ModelKind::Llm.file_name(), "model.gguf");
    }

    // The command defaults to the chat models, which is what every pre-existing
    // caller relies on.
    #[test]
    fn listing_defaults_to_llm_and_selects_by_kind() {
        assert!(list_model_options(None).iter().all(|m| m.kind == ModelKind::Llm));
        assert!(list_model_options(Some(ModelKind::Whisper)).iter().all(|m| m.kind == ModelKind::Whisper));
    }

    // The frontend sends the kind as a lowercase string; a rename of the variant
    // would break the wire format without touching any Rust call site.
    #[test]
    fn kind_serializes_as_lowercase() {
        assert_eq!(serde_json::to_string(&ModelKind::Llm).unwrap(), "\"llm\"");
        assert_eq!(serde_json::to_string(&ModelKind::Whisper).unwrap(), "\"whisper\"");
    }

    #[test]
    fn every_kind_remembers_its_install_under_its_own_key() {
        // The two kinds share the download code, so a single key would make the
        // whisper download overwrite which chat model is installed.
        let llm = ModelKind::Llm.installed_setting_key();
        let whisper = ModelKind::Whisper.installed_setting_key();
        assert_ne!(llm, whisper, "оба вида пишут в один ключ — установка одного затрёт другой");
        assert!(!llm.is_empty() && !whisper.is_empty());
    }

    #[test]
    fn the_file_name_cannot_identify_the_model() {
        // The reason the url has to be remembered at all: every chat model lands
        // in the same file, so the disk cannot answer "which one is installed?".
        // If this ever stops holding, the setting can go away — but it must be a
        // deliberate change, not a silent one.
        assert!(model_catalog().len() > 1, "в каталоге одна модель — тест бессмысленен");
        assert_eq!(
            ModelKind::Llm.file_name(), "model.gguf",
            "имя файла перестало быть фиксированным — пересмотреть, нужен ли installed_setting_key"
        );
    }
}
