# Kriptag

Local-first organiser with optional AI: tasks, notes and activity tracking in
one desktop application. Notes support Markdown, `[[wiki links]]` and a graph of
connections; tasks have a list and a board view, subtasks, deadlines and
recurrence. Activity tracking and a pomodoro timer feed a dashboard of where the
time actually went.

All data stays on this computer. AI features are optional and can run against a
local model.

Built with Tauri 2, Svelte 5 and SQLite.

## Building

Requires Node 20+ and a Rust toolchain.

```sh
npm ci
node scripts/fetch-sidecars.mjs   # see "Sidecars" below
npm run tauri dev                 # development
npx tauri build                   # installers in src-tauri/target/release/bundle/
```

On Linux the system dependencies are WebKitGTK and the tray library:

```sh
sudo apt-get install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
                     librsvg2-dev patchelf
```

### Sidecars

The AI features shell out to two bundled binaries, kept out of git because
llamafile alone is ~300 MB. `scripts/fetch-sidecars.mjs` downloads what it can:

- **llamafile** — fetched automatically. It is an APE binary, so the same file
  works on Linux and Windows.
- **whisper-cli** — *not* fetched. Upstream ships shared-library builds, and a
  Tauri sidecar has to be one self-contained file; the script prints the cmake
  command that produces a static one. Voice input is the only thing that needs
  it, and the app builds and runs without it.

A missing sidecar makes the bundler fail on purpose rather than silently ship a
broken package.

## Releases

`.github/workflows/release.yml` builds installers on a tag (`v*`) and attaches
them to a draft GitHub Release. It runs on two machines because Tauri draws the
interface with the *system* webview — WebKitGTK on Linux, WebView2 on Windows —
so a Windows installer cannot be cross-compiled from Linux and has to be built
on a Windows runner.

`workflow_dispatch` runs the same build without publishing, which is the way to
check the pipeline before tagging.

## Testing

```sh
cargo test --manifest-path src-tauri/Cargo.toml   # backend
npx vitest run                                    # pure frontend logic
npx playwright test                               # e2e against a mocked backend
npx svelte-check --threshold error
```

The e2e suite drives headless Chromium, while the application itself runs in
WebKitGTK — anything visual is worth confirming in a real build.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
