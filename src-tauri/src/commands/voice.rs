// Commands behind the microphone button: start recording, stop and get the text.
//
// Recording is a two-step operation rather than one blocking call because the user
// decides when to stop talking. The active recording lives in Tauri's managed
// state; there is at most one, since dictating into two places at once has no
// meaning and would fight over the same microphone.

use std::sync::Mutex;
use tauri::{AppHandle, State};

/// The recording in progress, if any.
pub struct VoiceState(pub Mutex<Option<crate::voice::audio::Recording>>);

impl VoiceState {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

/// Whether the microphone button should be shown at all.
#[tauri::command]
pub fn voice_available(app: AppHandle) -> bool {
    crate::voice::is_available(&app)
}

#[tauri::command]
pub async fn start_voice_recording(
    app: AppHandle,
    state: State<'_, VoiceState>,
) -> Result<(), String> {
    // Checked before touching the microphone: starting a recording that could
    // never be transcribed would waste the user's words.
    if !crate::voice::is_available(&app) {
        return Err("Голосовой ввод недоступен: не скачана модель распознавания.".into());
    }

    let mut slot = state.0.lock().map_err(|_| "Состояние записи повреждено")?;
    if slot.is_some() {
        return Err("Запись уже идёт.".into());
    }
    *slot = Some(crate::voice::audio::start()?);
    Ok(())
}

/// Stops the recording and returns the recognised text.
///
/// The WAV is written into the models directory rather than the system temp
/// folder, and that is a deliberate change from the obvious choice.
///
/// whisper is handed both files as command-line arguments, and on Windows it
/// cannot open a path containing non-ASCII characters (whisper.cpp issue #1955):
/// argv arrives in the system ANSI encoding and a Cyrillic user name turns into
/// mush. The fix is to name both files RELATIVELY, which requires them to share
/// one directory — and the model cannot move, so the recording does.
///
/// It is still an intermediate that whisper reads once, and it is still removed
/// even when recognition fails: a dictated phrase should not linger on disk.
#[tauri::command]
pub async fn stop_voice_recording(
    app: AppHandle,
    state: State<'_, VoiceState>,
) -> Result<String, String> {
    let recording = {
        let mut slot = state.0.lock().map_err(|_| "Состояние записи повреждено")?;
        slot.take().ok_or("Запись не была начата.")?
    };

    let wav = crate::commands::model::models_dir(&app)?
        .join(format!("kriptag-voice-{}.wav", uuid::Uuid::new_v4()));
    recording.stop_to_wav(&wav)?;

    let result = crate::voice::transcribe(&app, &wav).await;
    let _ = std::fs::remove_file(&wav);
    result
}

/// Drops the recording without transcribing — the user changed their mind.
#[tauri::command]
pub async fn cancel_voice_recording(state: State<'_, VoiceState>) -> Result<(), String> {
    let mut slot = state.0.lock().map_err(|_| "Состояние записи повреждено")?;
    // Dropping the Recording stops the stream; nothing was written to disk, so
    // there is no file to clean up.
    *slot = None;
    Ok(())
}
