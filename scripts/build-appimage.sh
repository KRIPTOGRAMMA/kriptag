#!/bin/sh
# Builds the AppImage in two steps, because `tauri build --bundles appimage`
# cannot finish it on this distribution.
#
# Two separate problems, found by running the pieces by hand:
#
# 1. linuxdeploy carries its own `strip` (a 2024 build). Arch's libraries have a
#    `.relr.dyn` section, which that strip does not know:
#      ERROR: Strip call failed: ... unknown type [0x13] section `.relr.dyn'
#    It fails on every library and the whole run dies. `NO_STRIP=1` skips
#    stripping and the errors disappear — confirmed, the system strip (binutils
#    2.47) handles the same file fine, so this is linuxdeploy's copy being old.
#
# 2. Even with NO_STRIP set, `tauri build` still reports "failed to run
#    linuxdeploy" while printing nothing from linuxdeploy itself. The AppDir it
#    produces is complete and correct: running linuxdeploy's packaging step
#    against that very AppDir succeeds and yields a working AppImage. So the
#    bundler's own invocation is what fails, for a reason it does not surface.
#
# Hence: let Tauri build the AppDir (it fails at the last step, which is fine),
# then finish the packaging ourselves.
set -e

cd "$(dirname "$0")/.."
OUT=src-tauri/target/release/bundle/appimage

echo "==> Step 1: Tauri builds the AppDir (its final step is expected to fail)"
NO_STRIP=1 npx tauri build --bundles appimage || true

APPDIR="$(find "$OUT" -maxdepth 1 -name '*.AppDir' | head -1)"
[ -n "$APPDIR" ] || { echo "No AppDir produced — the failure was earlier than usual." >&2; exit 1; }

echo "==> Step 2: packaging $APPDIR"
cd "$OUT"
NO_STRIP=1 "$HOME/.cache/tauri/linuxdeploy-x86_64.AppImage" \
  --appdir "$(basename "$APPDIR")" --output appimage

ls -la ./*.AppImage
