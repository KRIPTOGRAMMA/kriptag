#!/usr/bin/env node
// Fetches the sidecar binaries Tauri bundles into the package.
//
// They cannot live in git: llamafile alone is ~300 MB. `src-tauri/binaries/` is
// therefore in .gitignore, which until now meant a clean clone could not be
// built at all — the bundler fails hard on a missing sidecar (deliberately), and
// there was neither a script nor an instruction. CI hits exactly the same wall,
// because a runner always starts from a clean clone.
//
// Tauri resolves a sidecar by target triple: `binaries/llamafile` in the config
// becomes `binaries/llamafile-x86_64-unknown-linux-gnu` on Linux and
// `...-x86_64-pc-windows-msvc.exe` on Windows. The suffix is not decoration —
// the wrong name reads as "sidecar missing".

import { createWriteStream } from "node:fs";
import { mkdir, chmod, stat, writeFile, readFile } from "node:fs/promises";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";

// Collected so the script can end with a non-zero status when something a build
// actually needs is absent.
const missing = [];

const LLAMAFILE_VERSION = "0.10.5";
const WHISPER_VERSION = "v1.9.2";

const OUT = "src-tauri/binaries";

// The triple Tauri expects, and the extension that goes with it.
function hostTarget() {
  if (process.platform === "win32") return { triple: "x86_64-pc-windows-msvc", exe: ".exe" };
  if (process.platform === "darwin") return { triple: "x86_64-apple-darwin", exe: "" };
  return { triple: "x86_64-unknown-linux-gnu", exe: "" };
}

async function exists(path) {
  try { await stat(path); return true; } catch { return false; }
}

async function download(url, dest) {
  process.stdout.write(`  → ${url.split("/").pop()} `);
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText} for ${url}`);
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest));
  const { size } = await stat(dest);
  console.log(`(${(size / 1e6).toFixed(1)} MB)`);
}

// llamafile ships as an APE binary: one file that is a valid executable on both
// Linux and Windows, so the same download serves every platform. Verified by its
// MZ header — `file` reports it as "DOS/MBR boot sector" on Linux.
async function fetchLlamafile({ triple, exe }) {
  const dest = join(OUT, `llamafile-${triple}${exe}`);
  if (await exists(dest)) {
    // "Already here" says nothing about *which* version is here, and the file is
    // gitignored, so nothing else does either. That gap shipped a bug: this
    // machine kept a 0.9.3 left over from an earlier LLAMAFILE_VERSION and ran
    // it happily for months, while a clean build downloaded 0.10.5 — which had
    // dropped a flag the code still passed. Every local test passed; the Windows
    // package could not load a model at all.
    //
    // Asking the binary its version costs one exec and turns a silent skip into
    // a visible mismatch. It is not fixed automatically: the file may be a
    // deliberate local build, and deleting 300 MB of someone's work to re-fetch
    // it is not this script's call.
    let have = "unknown";
    try {
      const out = execFileSync(resolve(dest), ["--version"], {
        encoding: "utf8",
        timeout: 30_000,
        stdio: ["ignore", "pipe", "ignore"],
      });
      have = out.trim().split("\n")[0].replace(/^llamafile\s+v?/, "");
    } catch {
      // Unreadable or not executable — reported as a mismatch below, since an
      // unusable binary is worse than a stale one.
    }
    if (have === LLAMAFILE_VERSION) {
      console.log(`  = llamafile-${triple}${exe} already here (v${have})`);
    } else {
      console.log(
        `  ! llamafile-${triple}${exe} is v${have}, expected v${LLAMAFILE_VERSION}\n` +
        `    Flags differ between versions — delete it and re-run to match the build.`,
      );
    }
    return;
  }
  await download(
    `https://github.com/Mozilla-Ocho/llamafile/releases/download/${LLAMAFILE_VERSION}/llamafile-${LLAMAFILE_VERSION}`,
    dest,
  );
  await chmod(dest, 0o755);
}

// whisper.cpp is the awkward one. Upstream ships a *shared-library* build: the
// archive holds whisper-cli plus a dozen .so files (libwhisper, libggml and the
// per-CPU variants), and the binary refuses to start without them —
// "error while loading shared libraries: libwhisper.so.1". Tauri's sidecar
// mechanism copies single files next to the executable, so those libraries have
// nowhere to go, and the one currently in the repo is a 3.1 MB static build that
// upstream does not publish.
//
// Rather than download something that cannot work, the script says what is
// missing and how the working file was obtained. Silently producing a broken
// whisper-cli would be worse: the app would build and fail only when someone
// tries to dictate a note.
async function fetchWhisper({ triple, exe }) {
  const dest = join(OUT, `whisper-cli-${triple}${exe}`);
  if (await exists(dest)) { console.log(`  = whisper-cli-${triple}${exe} already here`); return; }

  console.log(`  ! whisper-cli-${triple}${exe} is missing and cannot be fetched automatically.`);
  console.log("    Upstream releases are shared-library builds; the sidecar has to be a");
  console.log("    single self-contained file. Build one statically:");
  console.log("");
  console.log("      git clone https://github.com/ggerganov/whisper.cpp");
  console.log("      cd whisper.cpp");
  console.log(`      git checkout ${WHISPER_VERSION}`);
  console.log("      cmake -B build -DBUILD_SHARED_LIBS=OFF -DCMAKE_BUILD_TYPE=Release \\");
  console.log("            -DGGML_NATIVE=OFF -DWHISPER_BUILD_TESTS=OFF");
  console.log("      cmake --build build -j --config Release");
  console.log("");
  console.log("    GGML_NATIVE=OFF matters: the default is -march=native, which bakes in");
  console.log("    whatever the building machine supports and dies with SIGILL elsewhere.");
  console.log(`      cp build/bin/whisper-cli ../${dest}`);
  console.log("");
  console.log("    Voice input is optional: everything else works without it.");
  missing.push(`whisper-cli-${triple}${exe}`);
}


// The wrapper exists only to find llamafile next to itself and exec it.
//
// It used to `cd /tmp` first, because llamafile writes into its working
// directory and the install folder may be read-only. That job now belongs to
// the caller: sidecar.rs sets current_dir to the models directory, which is
// always writable, and passes the model as a BARE RELATIVE NAME so that a
// Windows path containing spaces cannot be split by llamafile's own APE
// command-line parser. A `cd` here would move the process away from that
// directory and the relative name would resolve to nothing — the wrapper must
// therefore inherit the working directory it is given, not choose one.
//
// On Windows there is no wrapper at all. Tauri names a sidecar
// `<name>-<triple>.exe`, and a .exe has to be a real PE binary — a batch script
// under that name simply fails to start, which is what the first version of this
// script produced. Nothing is lost: the cwd trick is a Unix concern, and
// llamafile is an APE binary that Windows runs directly.
async function writeWrapper({ triple, exe }) {
  if (process.platform === "win32") {
    console.log("  = llamafile-wrapper not needed on Windows (llamafile runs directly)");
    return;
  }
  const dest = join(OUT, `llamafile-wrapper-${triple}${exe}`);
  if (await exists(dest)) { console.log(`  = llamafile-wrapper already here`); return; }
  // Both names are tried because the file is called different things at the two
  // moments this script has to serve. In the source tree the sidecar keeps its
  // target triple (`llamafile-x86_64-unknown-linux-gnu`), which is how `tauri
  // dev` runs it; when bundling, Tauri strips the triple and installs plain
  // `llamafile` next to the wrapper. A wrapper that knew only one of the two
  // worked in exactly one of those situations and reported "binary not found"
  // in the other.
  await writeFile(dest, [
    "#!/bin/sh",
    "# No `cd`: the caller sets the working directory and passes the model as a",
    "# relative name. See the comment above writeWrapper().",
    'DIR="$(dirname "$0")"',
    'if [ -f "$DIR/llamafile" ]; then',
    '  exec "$DIR/llamafile" "$@"',
    `elif [ -f "$DIR/llamafile-${triple}${exe}" ]; then`,
    `  exec "$DIR/llamafile-${triple}${exe}" "$@"`,
    "else",
    '  echo "llamafile binary not found" >&2',
    "  exit 1",
    "fi",
    "",
  ].join("\n"));
  await chmod(dest, 0o755);
  console.log(`  + llamafile-wrapper-${triple}${exe}`);
}

// externalBin lists files the bundler must find, and the wrapper is not one of
// them on Windows. Left in place it aborts the Windows build with a missing
// sidecar — the same way whisper-cli did.
async function dropEntry(match, why) {
  const confPath = "src-tauri/tauri.conf.json";
  const conf = JSON.parse(await readFile(confPath, "utf8"));
  const before = conf.bundle.externalBin.length;
  conf.bundle.externalBin = conf.bundle.externalBin.filter((b) => !b.includes(match));
  if (conf.bundle.externalBin.length !== before) {
    await writeFile(confPath, JSON.stringify(conf, null, 2) + "\n");
    console.log(`  ~ ${match} dropped from externalBin — ${why}.`);
  }
}

const target = hostTarget();
console.log(`Sidecars for ${target.triple}:`);
await mkdir(OUT, { recursive: true });
await fetchLlamafile(target);
await fetchWhisper(target);
await writeWrapper(target);

// externalBin is a list of files the bundler *must* find: a missing one aborts
// the build. whisper-cli is the only optional sidecar, so when it is absent the
// entry is dropped from the config rather than left to fail the build. The
// backend already handles its absence at runtime — voice.rs returns an error the
// UI shows — so the package is simply one without voice input.
if (missing.some((m) => m.startsWith("whisper-cli"))) {
  await dropEntry("whisper-cli", "building without voice input");
}
if (process.platform === "win32") {
  await dropEntry("llamafile-wrapper", "not used on Windows");
}

if (missing.length) {
  console.log(`\nMissing: ${missing.join(", ")} — see the note above.`);
}
console.log("Done.");
