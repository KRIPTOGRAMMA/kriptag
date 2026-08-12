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
import { mkdir, rm, readdir, rename, chmod, stat, writeFile } from "node:fs/promises";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { join } from "node:path";

const run = promisify(execFile);

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
  if (await exists(dest)) { console.log(`  = llamafile-${triple}${exe} already here`); return; }
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
  console.log("      cmake -B build -DBUILD_SHARED_LIBS=OFF -DCMAKE_BUILD_TYPE=Release");
  console.log("      cmake --build build -j --config Release");
  console.log(`      cp build/bin/whisper-cli ../${dest}`);
  console.log("");
  console.log("    Voice input is optional: the app builds and runs without it.");
  missing.push(`whisper-cli-${triple}${exe}`);
}

async function find(dir, name) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      const hit = await find(p, name);
      if (hit) return hit;
    } else if (entry.name === name) return p;
  }
  return null;
}

// The wrapper exists because llamafile insists on writing into the working
// directory; it is a shell script on Unix and a .cmd on Windows.
async function writeWrapper({ triple, exe }) {
  const dest = join(OUT, `llamafile-wrapper-${triple}${exe}`);
  if (await exists(dest)) { console.log(`  = llamafile-wrapper already here`); return; }
  if (process.platform === "win32") {
    await writeFile(dest, [
      "@echo off",
      `"%~dp0llamafile-${triple}${exe}" %*`,
      "",
    ].join("\r\n"));
  } else {
    await writeFile(dest, [
      "#!/bin/sh",
      'DIR="$(dirname "$0")"',
      "cd /tmp",
      `exec "$DIR/llamafile-${triple}${exe}" "$@"`,
      "",
    ].join("\n"));
  }
  await chmod(dest, 0o755);
  console.log(`  + llamafile-wrapper-${triple}${exe}`);
}

const target = hostTarget();
console.log(`Sidecars for ${target.triple}:`);
await mkdir(OUT, { recursive: true });
await fetchLlamafile(target);
await fetchWhisper(target);
await writeWrapper(target);

if (missing.length) {
  console.log(`\nMissing: ${missing.join(", ")} — see the note above.`);
  // Not an error exit: voice input is optional and the package builds without
  // it. CI decides for itself whether to treat this as fatal.
}
console.log("Done.");
