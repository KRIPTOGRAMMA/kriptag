// Voice input: dictating notes instead of typing them.
//
// The pipeline is deliberately batch rather than streaming — record, stop,
// transcribe — because whisper.cpp is a one-shot command over a finished file,
// not a server like llamafile. A dictated phrase is seconds long, so waiting for
// the end costs little and the code stays a great deal simpler.

pub mod audio;

use tauri::{AppHandle, Manager};
use tauri_plugin_shell::ShellExt;

/// The sidecar's name as tauri-plugin-shell expects it: a bare name, not a path.
///
/// It is resolved as `<directory of the executable>/<name>`, so a directory prefix
/// (`binaries/`, the layout in the sources and in tauri.conf.json) points at a
/// subdirectory that does not exist beside the built binary — the process then
/// fails to start with "No such file or directory".
const SIDECAR: &str = "whisper-cli";

/// Strips whisper's own decorations from the transcript.
///
/// Even with -np -nt the model emits bracketed annotations for non-speech
/// ([BLANK_AUDIO], [MUSIC], (wind blowing)) and pads the text with whitespace.
/// Inserting those into a note would be worse than inserting nothing.
pub fn clean_transcript(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth_square = 0usize;
    let mut depth_round = 0usize;
    for ch in raw.chars() {
        match ch {
            '[' => depth_square += 1,
            ']' => depth_square = depth_square.saturating_sub(1),
            '(' => depth_round += 1,
            ')' => depth_round = depth_round.saturating_sub(1),
            _ if depth_square == 0 && depth_round == 0 => out.push(ch),
            _ => {}
        }
    }
    // Whitespace is collapsed because removing an annotation leaves a gap behind,
    // and whisper indents every segment anyway.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Picks the line of whisper's stderr that actually explains the failure.
///
/// Taking the LAST non-empty line — the obvious choice, and the one that shipped
/// — is wrong here, because whisper prints its complaint first and then dumps its
/// entire usage text to stderr as well. On a missing input file that is 76 lines,
/// of which the last is `-vo N, --vad-samples-overlap ...`: a help entry shown to
/// the user as the cause of the error.
///
/// Every failure mode observed prefixes the real cause with `error:` and puts it
/// first, and the usage text contains no such prefix, so the first `error:` line
/// is the one to show.
///
/// `None` means whisper said nothing at all, which is itself diagnostic: the
/// process died before it could write, the way a missing DLL or an illegal
/// instruction kills it. The caller reports the exit status instead, since
/// "unknown error" tells the user nothing they can act on.
pub fn failure_reason(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("error:"))
        .map(|l| l.trim_start_matches("error:").trim().to_string())
        .filter(|l| !l.is_empty())
}

/// Whether voice input can run at all: both the model and the sidecar must exist.
///
/// The same capability detection the project already applies to window tracking
/// and notification actions — the button simply does not appear when dictation
/// cannot work, rather than appearing and failing.
///
/// The sidecar is checked for EXISTENCE, not merely for a successful lookup.
/// `sidecar()` resolves to `<directory of the executable>/<name>` and returns
/// `Ok` for a name that is not on disk at all — reading the plugin's source
/// (`relative_command_path`) confirms it only joins paths and never calls
/// `exists()`. So `.is_ok()` was true unconditionally, the microphone button
/// appeared on builds carrying no whisper-cli, and dictation failed only after
/// the user had finished speaking.
///
/// That case is real rather than hypothetical on Windows: scripts/fetch-sidecars.mjs
/// cannot fetch whisper-cli there and drops it from externalBin, so the packaged
/// app legitimately ships without it.
pub fn is_available(app: &AppHandle) -> bool {
    crate::commands::model::model_available(app, crate::commands::model::ModelKind::Whisper)
        && sidecar_present()
}

/// Whether the whisper-cli sidecar is actually on disk beside the executable.
///
/// The plugin does not expose the path it resolves, so the same arithmetic is
/// repeated here: `<directory of the executable>/<name>`. Duplicating it is the
/// lesser evil — the alternative is trusting a lookup that cannot fail.
fn sidecar_present() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(sidecar_file_name())))
        .map(|path| path.exists())
        .unwrap_or(false)
}

/// The sidecar's file name as it lands beside the executable.
///
/// Windows needs the `.exe` the plugin appends when resolving the name; without
/// it the existence check would look for a file that is never there and report
/// dictation as unavailable on the one platform it was written to protect.
fn sidecar_file_name() -> String {
    if cfg!(windows) {
        format!("{SIDECAR}.exe")
    } else {
        SIDECAR.to_string()
    }
}

/// Runs whisper over a finished WAV and returns the recognised text.
///
/// The flags are chosen so stdout carries the transcript and nothing else:
/// `-np` suppresses the progress and system information, `-nt` the timestamps.
///
/// The language is left on `auto` rather than following the interface language:
/// the two are unrelated — the author of this app runs a Russian interface and
/// dictates in both languages — and forcing the wrong one produces confident
/// nonsense rather than an error (verified: forcing `-l ru` on English speech
/// returned a mangled half-translation).
pub async fn transcribe(app: &AppHandle, wav: &std::path::Path) -> Result<String, String> {
    let model = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models")
        .join(crate::commands::model::ModelKind::Whisper.file_name());

    if !model.exists() {
        return Err("Модель распознавания не найдена. Скачайте её в настройках.".into());
    }

    let output = app
        .shell()
        .sidecar(SIDECAR)
        .map_err(|e| format!("Не удалось найти whisper-cli: {e}"))?
        .args([
            "-m", model.to_str().ok_or("Некорректный путь к модели")?,
            "-f", wav.to_str().ok_or("Некорректный путь к записи")?,
            "-l", "auto",
            "-np",
            "-nt",
        ])
        .output()
        .await
        .map_err(|e| format!("Не удалось запустить распознавание: {e}"))?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The exit status alone is not a reliable verdict: an unknown argument makes
    // whisper print `error: unknown argument` and exit 0 (verified), so a failure
    // would slip through as an empty transcript and be reported as unclear
    // speech. Anything it calls an error is therefore a failure regardless.
    let reason = failure_reason(&stderr);
    if !output.status.success() || reason.is_some() {
        return Err(match reason {
            Some(reason) => format!("Распознавание не удалось: {reason}"),
            // Silence is the informative case on Windows: a process killed by a
            // missing DLL or an illegal instruction never gets to write. The
            // status is all there is, and it beats "unknown error", which gave
            // the user nothing to report and nobody anything to debug.
            None => format!(
                "Распознавание не удалось: whisper-cli завершился без сообщения ({}). \
                 Обычно это значит, что программу распознавания не удалось запустить.",
                // No code at all means the process was killed rather than having
                // exited — worth distinguishing, since that is what a crash on
                // startup looks like.
                match output.status.code() {
                    Some(code) => format!("код возврата {code}"),
                    None => "прерван".to_string(),
                }
            ),
        });
    }

    let text = clean_transcript(&String::from_utf8_lossy(&output.stdout));
    if text.is_empty() {
        return Err("Не удалось разобрать речь — попробуйте ещё раз.".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // whisper marks silence with [BLANK_AUDIO]; pasting that into a note would be
    // worse than pasting nothing.
    #[test]
    fn bracketed_annotations_are_removed() {
        assert_eq!(clean_transcript(" [BLANK_AUDIO]"), "");
        assert_eq!(clean_transcript("[MUSIC] привет"), "привет");
        assert_eq!(clean_transcript("привет (wind blowing) мир"), "привет мир");
    }

    #[test]
    fn leading_whitespace_and_padding_go_away() {
        assert_eq!(clean_transcript("  Купить хлеб.  "), "Купить хлеб.");
        assert_eq!(clean_transcript("строка\n  вторая"), "строка вторая");
    }

    // Real speech must survive untouched — the cleaner must not eat punctuation or
    // ordinary words.
    #[test]
    fn plain_speech_is_preserved() {
        let s = "Позвонить Ивану в 15:00, обсудить бюджет — это важно!";
        assert_eq!(clean_transcript(s), s);
    }

    // The recognised text may legitimately contain brackets the user dictated; an
    // unbalanced one must not swallow the rest of the phrase.
    #[test]
    fn unbalanced_closing_bracket_does_not_swallow_the_text() {
        assert_eq!(clean_transcript("текст] дальше"), "текст дальше");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(clean_transcript(""), "");
        assert_eq!(clean_transcript("   \n  "), "");
    }

    // The stderr whisper really produces when the input file is missing: the
    // complaint first, then its entire usage text — 76 lines, of which this is a
    // faithful excerpt including the final one.
    const MISSING_FILE_STDERR: &str = "\
error: input file not found '/nope.wav'
error: no input files specified

usage: whisper-cli [options] file0 file1 ...

options:
  -h,        --help                 [default] show this help message and exit
  -vo N,     --vad-samples-overlap         N [0.10   ] VAD samples overlap (seconds between segments)
";

    // The bug this replaces: the code took the LAST non-empty line, which is a
    // help entry about VAD overlap, and showed it to the user as the cause.
    #[test]
    fn the_cause_is_taken_from_the_error_line_not_the_usage_text() {
        let reason = failure_reason(MISSING_FILE_STDERR).expect("причина не найдена");
        assert_eq!(reason, "input file not found '/nope.wav'");
        assert!(
            !reason.contains("vad-samples-overlap"),
            "показана строка справки вместо ошибки: {reason}"
        );
    }

    // Silence is a distinct, meaningful case — a process killed before it could
    // write — and must be reported as such rather than as some fabricated cause.
    #[test]
    fn silent_stderr_yields_no_reason() {
        assert_eq!(failure_reason(""), None);
        assert_eq!(failure_reason("   \n\n  "), None);
        // Output without a complaint is not a cause either: whisper narrates its
        // progress on stderr even on the successful path.
        assert_eq!(
            failure_reason("read_audio_data: reading audio data from 'x.wav' ..."),
            None
        );
    }

    // Whisper's own failures, verified by running it: exit 3 on a corrupt model,
    // exit 0 on an unknown argument.
    #[test]
    fn real_whisper_failures_are_reported_verbatim() {
        assert_eq!(
            failure_reason("error: failed to initialize whisper context").as_deref(),
            Some("failed to initialize whisper context")
        );
        assert_eq!(
            failure_reason("error: unknown argument: --bogus-flag").as_deref(),
            Some("unknown argument: --bogus-flag")
        );
    }

    // An unknown argument exits 0 while printing an error, so a status-only check
    // would treat a failed run as success and blame the user's speech. The guard
    // is on the source because the branch needs an AppHandle to reach otherwise.
    #[test]
    fn a_failure_is_not_judged_by_the_exit_status_alone() {
        let src = include_str!("mod.rs");
        let production = src
            .split(concat!("#[cfg(", "test)]"))
            .next()
            .expect("не найден производственный код");
        assert!(
            production.contains("!output.status.success() || reason.is_some()"),
            "провал определяется только кодом возврата — whisper печатает \
             `error: unknown argument` и выходит с нулём, и такой прогон \
             будет принят за успех"
        );
    }

    // The sidecar name is a name, not a path. tauri-plugin-shell builds the command
    // as `<directory of the executable>/<name>` (relative_command_path), so a name
    // carrying a directory prefix points at a subdirectory that does not exist
    // beside the binary, and the process fails to start with "No such file or
    // directory". The directory in tauri.conf.json is the layout in the sources;
    // beside the built application there is none.
    //
    // Found only by running the app — nothing in the suite covered it. The test
    // repeats the same path arithmetic. Matching against the source text is not an
    // option: such a check trips over its own comment (tried).
    // `sidecar()` only joins paths — reading tauri-plugin-shell's own
    // `relative_command_path` confirms it never calls exists() — so `.is_ok()`
    // was true even on a build carrying no whisper-cli. The microphone button
    // appeared and dictation failed only after the user had finished speaking.
    // On Windows that build is the normal one: fetch-sidecars.mjs cannot fetch
    // whisper-cli and drops it from externalBin.
    //
    // The guard is on the ANSWER, not on the mechanism: whatever the
    // implementation, availability must not be claimable without a file on disk.
    #[test]
    fn availability_requires_the_sidecar_to_exist_on_disk() {
        // The test binary's own directory has no whisper-cli beside it, so a
        // truthful check must say no here.
        assert!(
            !sidecar_present(),
            "сайдкар «найден» там, где его нет — проверка не смотрит на диск"
        );
    }

    // The name the existence check looks for must be the name the plugin resolves,
    // including the .exe it appends on Windows. Getting this wrong would report
    // dictation as unavailable on the very platform the check was added for.
    #[test]
    fn the_checked_file_name_matches_what_the_plugin_resolves() {
        let name = sidecar_file_name();
        assert!(name.starts_with(SIDECAR), "проверяется чужое имя: {name}");
        assert_eq!(name.ends_with(".exe"), cfg!(windows), "расширение не по платформе: {name}");
    }

    #[test]
    fn sidecar_name_resolves_next_to_the_executable() {
        let exe_dir = std::path::Path::new("/opt/app");
        assert_eq!(exe_dir.join(SIDECAR), std::path::Path::new("/opt/app/whisper-cli"));
        assert_eq!(
            exe_dir.join("binaries/whisper-cli"),
            std::path::Path::new("/opt/app/binaries/whisper-cli"),
            "this is the path that did not exist"
        );
    }
}
