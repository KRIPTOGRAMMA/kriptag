use std::collections::VecDeque;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};

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
/// The wrapper is a Unix shell script that only locates llamafile beside itself
/// and execs it. It used to `cd /tmp` as well, because llamafile writes into its
/// working directory; that is now the caller's job below, which sets the models
/// directory explicitly and needs the wrapper to leave it alone.
/// On Windows the wrapper does not exist — a sidecar there
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

    // Taken from ModelKind rather than spelled out, because this name is passed
    // to llamafile as a bare relative argument below: a literal here that drifted
    // from the one download_model() writes would look like a missing model.
    let model_file = crate::commands::model::ModelKind::Llm.file_name();

    let model_path = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models")
        .join(model_file);

    if !model_path.exists() {
        return Err("Модель не найдена. Скачайте модель в настройках.".into());
    }

    // The directory the model lives in, and from now on the working directory
    // llamafile runs in. Both the `-m` argument and `current_dir` are derived
    // from it, so they cannot drift apart.
    let models_dir = model_path
        .parent()
        .ok_or_else(|| "путь к модели без родительской папки".to_string())?
        .to_path_buf();

    let port = pick_free_port();

    let sidecar = app
        .shell()
        .sidecar(SIDECAR)
        .map_err(|e| format!("sidecar lookup failed: {e}"))?
        // Run from the models directory, so the model can be named relatively
        // below. Set BEFORE the arguments purely so the two read together.
        .current_dir(models_dir)
        // Deliberately only the flags 0.10.5 still accepts.
        //
        // `--nobrowser` used to be here and is why the first Windows package
        // could not start the model at all: llamafile removed the flag, and an
        // unknown argument is fatal — "error: invalid argument: --nobrowser",
        // exit 256, before the model is even opened. It went unnoticed locally
        // because the binaries directory is gitignored and the fetch script
        // skips a file that is already there, so this machine kept an old 0.9.3
        // that still understood the flag while the build shipped 0.10.5.
        //
        // Nothing replaces it: `--server` in 0.10.5 is the API server and opens
        // no browser tab. Verified against the real binary, not the changelog.
        //
        // ---
        //
        // The model is named RELATIVELY, and that is the whole point.
        //
        // llamafile is an APE binary: on Windows it does not receive a parsed
        // argv from the OS, it re-parses the raw command line itself, splitting
        // on whitespace the Unix way. An absolute Windows path is
        // `C:\Users\<name>\AppData\Roaming\com.kriptag.app\models\model.gguf`,
        // and the moment the account name contains a space it arrives as two
        // arguments — `-m` gets only the first half and the model "is not
        // found" even though it is plainly on disk. Backslashes are the second
        // hazard: to that parser they read as escapes, not separators.
        //
        // A bare filename has neither spaces nor separators, so there is
        // nothing left to misparse. `current_dir` above is what makes it
        // resolve, and both come from the same `models_dir`.
        //
        // Note this cannot be reproduced on Linux — the paths there have no
        // spaces and no backslashes, which is exactly why it shipped.
        .args([
            "--server",
            "--port", &port.to_string(),
            "-m", model_file,
        ]);

    let (mut events, child) = sidecar.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    // The last lines llamafile printed before it stopped being interesting.
    //
    // This receiver used to be dropped into `_`, and that is why the first
    // Windows package could only report "did not start within 60 seconds"
    // without saying why: llamafile explains its refusals on stderr, and there
    // was nobody reading them. The plugin's channel holds one message, so a
    // dropped receiver makes every send fail silently — the process runs, the
    // diagnosis is thrown away.
    //
    // Bounded on purpose: llamafile logs the whole model metadata on startup
    // (dozens of lines), and keeping all of it would be a slow leak for a
    // long-running server. The tail is what a failure needs.
    let log: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    let log_writer = log.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            let line = match event {
                CommandEvent::Stderr(b) | CommandEvent::Stdout(b) => {
                    String::from_utf8_lossy(&b).trim_end().to_string()
                }
                CommandEvent::Error(e) => format!("error: {e}"),
                CommandEvent::Terminated(payload) => {
                    format!("terminated: code={:?}", payload.code)
                }
                _ => continue,
            };
            if line.is_empty() {
                continue;
            }
            let mut l = log_writer.lock().unwrap();
            if l.len() == SIDECAR_LOG_LINES {
                l.pop_front();
            }
            l.push_back(line);
        }
    });

    {
        let mut s = state.lock().unwrap();
        s.child = Some(child);
        s.port = port;
        s.ready = false;
    }

    if let Err(e) = wait_for_ready(port).await {
        // The tail goes into the message the user actually sees. Without it the
        // error names a symptom and hides the cause.
        let tail = log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
        return Err(if tail.is_empty() {
            format!("{e}\nllamafile ничего не написал в вывод.")
        } else {
            format!("{e}\n\nПоследние строки llamafile:\n{tail}")
        });
    }

    {
        let mut s = state.lock().unwrap();
        s.ready = true;
    }

    Ok(port)
}

/// How long a model is given to load.
///
/// Measured, not guessed: a 2.4 GB Phi-3.5 comes up in about 6 seconds here with
/// a warm page cache. The budget is for the case that is not warm — a multi-
/// gigabyte file read from disk for the first time, on Windows with an antivirus
/// scanning both the model and an APE binary it has never seen. The old 60 was
/// too tight for exactly that, and it is what the first Windows package hit.
const STARTUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(300);

/// How many trailing lines of llamafile's own output are kept for diagnosis.
const SIDECAR_LOG_LINES: usize = 40;

async fn wait_for_ready(port: u16) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{}/v1/models", port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();

    // By the clock rather than by iteration count. Counting iterations made the
    // message lie about its own condition: each probe costs up to the 2s request
    // timeout PLUS the 1s sleep, so "60 tries" was anywhere between 60 and 180
    // seconds depending on whether the port refused instantly or hung.
    let deadline = std::time::Instant::now() + STARTUP_BUDGET;
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err(format!(
        "llamafile не ответил за {} секунд",
        STARTUP_BUDGET.as_secs()
    ))
}

#[cfg(test)]
mod tests {
    use super::{SIDECAR, STARTUP_BUDGET};

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

    // Guards the three things that made the first Windows package fail without
    // saying why. All read the source: there is no way to spawn a real llamafile
    // in a unit test, and a shape check here is still worth more than nothing —
    // each of these was a live bug, not a hypothetical.
    /// The production half of this file.
    ///
    /// include_str! reads the whole file, tests included, so a guard that looks
    /// for a literal finds its own text and passes on broken code. Two of the
    /// four guards below did exactly that before this split — verified by
    /// breaking them.
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("sidecar.rs");
        // The marker itself is written in two pieces so this line does not become
        // the first match and cut the file at zero.
        SRC.split(concat!("#[cfg(", "test)]")).next().unwrap_or("")
    }

    #[test]
    fn the_sidecars_own_output_is_read() {
        // `let (_, child) = spawn()` drops the event receiver, and the plugin's
        // channel holds one message — every line llamafile prints then fails to
        // send silently. That is why the failure could only be reported as a
        // timeout with no cause.
        // Looking at what spawn() is bound to, not at a literal spelling of the
        // broken line: the broken form would otherwise have to be written out
        // here, and include_str! reads this file too — the guard would match its
        // own text and fail on correct code. It did, on the first attempt.
        let binding = production_src()
            .split("sidecar.spawn()")
            .next()
            .and_then(|before| before.rsplit("let ").next())
            .unwrap_or("");
        assert!(
            !binding.starts_with("(_,"),
            "приёмник событий выброшен в `_` — вывод llamafile потеряется, \
             и падение опять нельзя будет объяснить; получено: {binding:?}"
        );
        assert!(
            production_src().contains("events.recv()"),
            "события сайдкара никто не читает: именно там llamafile пишет причину отказа"
        );
    }

    #[test]
    fn the_startup_error_carries_what_llamafile_said() {
        assert!(
            production_src().contains("Последние строки llamafile"),
            "сообщение об ошибке не несёт вывод сайдкара — остаётся симптом без причины"
        );
    }

    #[test]
    fn readiness_is_measured_by_the_clock_not_by_iterations() {
        // Each probe costs up to the request timeout PLUS the sleep, so a loop
        // counting iterations claimed "60 seconds" while actually allowing
        // anywhere from 60 to 180. The message has to match the condition.
        assert!(
            production_src().contains("let deadline = std::time::Instant::now()"),
            "ожидание снова считается итерациями — сообщение о таймауте будет врать"
        );
        assert!(
            STARTUP_BUDGET.as_secs() >= 120,
            "бюджет старта {} с — мало для многогигабайтной модели при холодном чтении с диска",
            STARTUP_BUDGET.as_secs()
        );
    }

    #[test]
    fn every_flag_passed_to_llamafile_still_exists() {
        // The bug this file exists to prevent, in its most recent form: llamafile
        // dropped `--nobrowser` between 0.9.3 and 0.10.5, an unknown argument is
        // fatal rather than ignored, and the Windows package therefore died with
        // "error: invalid argument: --nobrowser" before touching the model.
        //
        // The allow-list is small on purpose and every entry was run against the
        // real 0.10.5 binary. When the sidecar version is bumped, this test is
        // the place that has to be re-checked the same way — the point is that
        // adding a flag from memory cannot pass silently.
        const KNOWN_GOOD: [&str; 3] = ["--server", "--port", "-m"];

        let args = production_src()
            .split(".args([")
            .nth(1)
            .and_then(|rest| rest.split("]);").next())
            .expect("не найден список аргументов сайдкара");

        for flag in args
            .split('"')
            .filter(|s| s.starts_with('-') && !s.contains(char::is_whitespace))
        {
            assert!(
                KNOWN_GOOD.contains(&flag),
                "флаг {flag} не подтверждён на llamafile 0.10.5 — неизвестный аргумент \
                 фатален, модель не запустится вообще; проверьте на живом бинарнике \
                 и добавьте в KNOWN_GOOD"
            );
        }
    }

    #[test]
    fn the_model_is_passed_as_a_bare_relative_name() {
        // llamafile re-parses the raw Windows command line itself, splitting on
        // whitespace, so an absolute path under `C:\Users\<name with a space>`
        // reaches `-m` cut in half and the model reads as missing while sitting
        // plainly on disk. Linux paths have neither spaces nor backslashes,
        // which is why this can never be caught by running it here.
        //
        // The guard is on the ARGUMENT, not on the presence of some helper:
        // whatever the mechanism, what must stay true is that nothing
        // path-shaped goes onto that command line.
        let args = production_src()
            .split(".args([")
            .nth(1)
            .and_then(|rest| rest.split("]);").next())
            .expect("не найден список аргументов сайдкара");

        let model_arg = args
            .split("\"-m\"")
            .nth(1)
            .expect("аргумент -m не найден")
            .split(',')
            .nth(1)
            .expect("у -m нет значения");

        assert!(
            !model_arg.contains("model_path") && !model_arg.contains("app_data_dir"),
            "путь к модели передаётся абсолютным ({}) — на Windows пробел в имени \
             пользователя разрежет его на два аргумента и модель «не найдётся»; \
             передавайте имя файла относительно current_dir",
            model_arg.trim()
        );
    }

    #[test]
    fn the_unix_wrapper_keeps_the_working_directory_it_is_given() {
        // The relative `-m` above only resolves if nothing moves the process
        // afterwards. The wrapper used to `cd /tmp` for its own reasons, which
        // would now silently break model loading on Linux — the opposite
        // platform from the one the relative path was introduced for.
        let script = include_str!("../../../scripts/fetch-sidecars.mjs");
        let body = script
            .split("async function writeWrapper")
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .expect("не найдена функция writeWrapper");

        assert!(
            !body.contains("\"cd "),
            "обёртка меняет рабочий каталог — относительный путь к модели \
             перестанет разрешаться, и модель не загрузится на Linux"
        );
    }

    #[test]
    fn the_working_directory_is_set_explicitly() {
        // llamafile writes into its cwd. On Windows there is no wrapper to move
        // out of Program Files first, and inheriting the app's cwd puts writes
        // somewhere the user may not be allowed to write.
        assert!(
            production_src().contains("current_dir"),
            "рабочий каталог не задан: на Windows llamafile будет писать в папку установки"
        );
    }
}
