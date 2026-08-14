use std::net::TcpListener;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::CommandChild;

pub struct SidecarState {
    pub child: Option<CommandChild>,
    pub port: u16,
    pub ready: bool,
}

impl SidecarState {
    pub fn new() -> Self {
        Self { child: None, port: 0, ready: false }
    }
}

pub type SharedSidecar = Mutex<SidecarState>;

/// The sidecar's name as tauri-plugin-shell expects it: a bare name, not a path.
/// It resolves as `<directory of the executable>/<name>`, so a directory prefix
/// points at a subdirectory that does not exist beside the built binary. The
/// `binaries/` prefix in tauri.conf.json is the layout in the SOURCES and is
/// correct there — it must not be repeated here. See voice/mod.rs, where the same
/// mistake broke speech recognition outright.
///
/// The wrapper is a Unix shell script whose only job is `cd /tmp`: llamafile
/// writes into the working directory, which beside the installed binary is either
/// read-only or the wrong place. On Windows it does not exist — a sidecar there
/// must be a real PE binary under a `.exe` name, so scripts/fetch-sidecars.mjs
/// neither writes one nor leaves it in externalBin. Asking for it anyway is what
/// made the model unusable in the first Windows package: the file is simply not
/// in the bundle, and the spawn fails with "os error 2". llamafile is an APE
/// binary that Windows runs directly, so it is called without the wrapper.
#[cfg(not(target_os = "windows"))]
const SIDECAR: &str = "llamafile-wrapper";
#[cfg(target_os = "windows")]
const SIDECAR: &str = "llamafile";

fn pick_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("failed to bind")
        .local_addr()
        .unwrap()
        .port()
}

pub async fn ensure_running(app: &AppHandle, state: &SharedSidecar) -> Result<u16, String> {
    // If already ready, return port immediately.
    let (already_ready, already_started, existing_port) = {
        let s = state.lock().unwrap();
        (s.ready, s.child.is_some(), s.port)
    };

    if already_ready {
        return Ok(existing_port);
    }
    if already_started {
        return wait_for_ready(existing_port).await.map(|_| existing_port);
    }

    let model_path = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models/model.gguf");

    if !model_path.exists() {
        return Err("Модель не найдена. Скачайте модель в настройках.".into());
    }

    let port = pick_free_port();

    let sidecar = app
        .shell()
        .sidecar(SIDECAR)
        .map_err(|e| format!("sidecar lookup failed: {e}"))?
        .args([
            "--server",
            "--port", &port.to_string(),
            "--nobrowser",
            "-m", model_path.to_str().unwrap(),
        ]);

    let (_, child) = sidecar.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    {
        let mut s = state.lock().unwrap();
        s.child = Some(child);
        s.port = port;
        s.ready = false;
    }

    wait_for_ready(port).await?;

    {
        let mut s = state.lock().unwrap();
        s.ready = true;
    }

    Ok(port)
}

async fn wait_for_ready(port: u16) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{}/v1/models", port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();

    for _i in 0..60 {
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => return Ok(()),
            _ => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
        }
    }
    Err("llamafile did not start within 60 seconds".into())
}

#[cfg(test)]
mod tests {
    use super::SIDECAR;

    // Guards EVERY sidecar call in the project, not just this file's.
    //
    // The mistake has now been made twice: it broke speech recognition (fixed by
    // giving voice/mod.rs a bare-name constant) and then sat unnoticed in this
    // file, where it made the local LLM unable to start in any built package. The
    // dev profile hides it — target/debug/ happens to contain BOTH a binaries/
    // subdirectory and the flat files, so the prefixed path resolves there. In
    // target/release/ and inside the .deb the layout is flat only.
    //
    // The guard written alongside the first fix only checked its own constant,
    // which is why the neighbour survived. This one reads the sources.
    //
    // Note the needle is assembled at runtime from two halves: a literal spelled
    // out in full would make this test match its own text, a trap already sprung
    // once when include_str! found the pattern inside a comment.
    #[test]
    fn no_sidecar_call_carries_a_directory_prefix() {
        let sources = [
            ("ai/sidecar.rs", include_str!("sidecar.rs")),
            ("voice/mod.rs", include_str!("../voice/mod.rs")),
        ];
        let needle = format!(".{}(\"", "sidecar");

        let mut offenders: Vec<String> = Vec::new();
        for (name, src) in sources {
            for (n, line) in src.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue; // prose, not a call
                }
                let Some(at) = line.find(&needle) else { continue };
                let rest = &line[at + needle.len()..];
                let Some(end) = rest.find('"') else { continue };
                let arg = &rest[..end];
                if arg.contains('/') {
                    offenders.push(format!("{name}:{}  {}", n + 1, line.trim()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a sidecar name must be bare; it resolves next to the executable:\n{}",
            offenders.join("\n")
        );
    }

    /// The name has to exist in the bundle, not merely be well-formed.
    ///
    /// The guard above checks the SHAPE of a sidecar name and passed happily while
    /// the first Windows package could not run the model at all: it asked for
    /// `llamafile-wrapper`, a Unix shell script that scripts/fetch-sidecars.mjs
    /// deliberately does not produce on Windows and strips from externalBin. A
    /// correctly-spelled name for a file that is not shipped fails exactly like a
    /// misspelled one — "os error 2" — which is why form alone was not enough.
    ///
    /// tauri.conf.json is the source of truth here because it is what the bundler
    /// reads. Note the fetch script REMOVES entries from it at build time (the
    /// wrapper on Windows, whisper-cli when it cannot be built), so this asserts
    /// the opposite direction too: the name this file asks for must survive that
    /// editing on the platform being compiled for.
    #[test]
    fn sidecar_name_is_shipped_on_this_platform() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).expect("tauri.conf.json");
        let entries: Vec<String> = conf["bundle"]["externalBin"]
            .as_array()
            .expect("bundle.externalBin missing")
            .iter()
            .map(|e| e.as_str().unwrap_or_default().to_string())
            .collect();

        // externalBin carries the source-tree prefix (`binaries/llamafile`) while
        // SIDECAR is the bare resolved name; compare on the last segment.
        let shipped: Vec<&str> = entries
            .iter()
            .map(|e| e.rsplit('/').next().unwrap_or(e))
            .collect();

        assert!(
            shipped.contains(&SIDECAR),
            "sidecar {SIDECAR:?} is not in externalBin {shipped:?} — it will not be in the \
             package and the spawn fails with os error 2"
        );

        // The wrapper is a Unix-only shell script. Asking for it on Windows is the
        // exact bug this test exists for, and it would otherwise pass whenever the
        // entry is still listed in the unedited config.
        #[cfg(target_os = "windows")]
        assert_ne!(
            SIDECAR, "llamafile-wrapper",
            "the wrapper is a shell script and is never bundled on Windows"
        );
    }
}
