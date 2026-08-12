use std::path::PathBuf;
use base64::Engine;
use tauri::Manager;
use crate::error::{AppError, AppResult};

const ALLOWED_EXT: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

fn normalize_ext(ext: &str) -> Option<&'static str> {
    ALLOWED_EXT.iter().find(|e| e.eq_ignore_ascii_case(ext)).copied()
}

// Decodes base64 (with an optional data: prefix, "data:image/png;base64,...") and
// writes the file to images_dir/<uuid>.<ext>. Returns only the relative filename:
// the frontend does not need an absolute path, it resolves one via
// convertFileSrc plus app_data_dir.
pub fn save_note_image_impl(images_dir: &std::path::Path, data_base64: &str, ext: &str) -> AppResult<String> {
    let ext = normalize_ext(ext.trim().trim_start_matches('.'))
        .ok_or_else(|| AppError::Other(format!("Недопустимое расширение: {ext}")))?;

    let payload = match data_base64.split_once(",") {
        Some((prefix, rest)) if prefix.starts_with("data:") => rest,
        _ => data_base64,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| AppError::Other(format!("Некорректный base64: {e}")))?;

    std::fs::create_dir_all(images_dir)?;
    let filename = format!("{}.{}", uuid::Uuid::new_v4(), ext);
    let path: PathBuf = images_dir.join(&filename);
    std::fs::write(path, bytes)?;

    Ok(filename)
}

#[tauri::command]
pub async fn save_note_image(
    app: tauri::AppHandle,
    data_base64: String,
    ext: String,
) -> AppResult<String> {
    let images_dir = app.path().app_data_dir()?.join("images");
    save_note_image_impl(&images_dir, &data_base64, &ext)
}

// The absolute path to the images folder. The frontend feeds it to
// convertFileSrc() to build asset:// URLs (the scope in tauri.conf.json is
// limited to this folder).
#[tauri::command]
pub fn get_images_dir(app: tauri::AppHandle) -> AppResult<String> {
    let dir = app.path().app_data_dir()?.join("images");
    Ok(dir.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_plain_base64_with_valid_ext() {
        let dir = std::env::temp_dir().join(format!("kriptag-test-{}", uuid::Uuid::new_v4()));
        let data = base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes");
        let name = save_note_image_impl(&dir, &data, "png").unwrap();
        assert!(name.ends_with(".png"));
        assert!(dir.join(&name).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_data_url_prefixed_base64() {
        let dir = std::env::temp_dir().join(format!("kriptag-test-{}", uuid::Uuid::new_v4()));
        let data = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes")
        );
        let name = save_note_image_impl(&dir, &data, "png").unwrap();
        assert!(dir.join(&name).exists());
        let saved = std::fs::read(dir.join(&name)).unwrap();
        assert_eq!(saved, b"fake-png-bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unknown_extension() {
        let dir = std::env::temp_dir().join(format!("kriptag-test-{}", uuid::Uuid::new_v4()));
        let data = base64::engine::general_purpose::STANDARD.encode(b"x");
        let r = save_note_image_impl(&dir, &data, "exe");
        assert!(r.is_err());
    }

    #[test]
    fn rejects_invalid_base64() {
        let dir = std::env::temp_dir().join(format!("kriptag-test-{}", uuid::Uuid::new_v4()));
        let r = save_note_image_impl(&dir, "not-base64!!!", "png");
        assert!(r.is_err());
    }

    #[test]
    fn extension_case_insensitive_and_dot_stripped() {
        let dir = std::env::temp_dir().join(format!("kriptag-test-{}", uuid::Uuid::new_v4()));
        let data = base64::engine::general_purpose::STANDARD.encode(b"x");
        let name = save_note_image_impl(&dir, &data, ".JPG").unwrap();
        assert!(name.ends_with(".jpg"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
