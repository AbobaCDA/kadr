#!/usr/bin/env bash
# Builds the Windows installer for this fork, as small as the stack allows.
#
#   tools/build-windows.sh            # NSIS installer (.exe) for the current arch
#   tools/build-windows.sh --no-bundle # just the exe, no installer
#
# Prerequisites (one time, see docs/BUILD.md):
#   - Rust stable (MSVC toolchain): https://rustup.rs
#   - Visual Studio Build Tools with "Desktop development with C++"
#   - Node.js 20+ (or Bun)
#   - WebView2 is already present on Windows 10/11 (the installer can bundle it
#     if you need to support older machines)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

NO_BUNDLE=0
for arg in "$@"; do
  case "$arg" in
    --no-bundle) NO_BUNDLE=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

echo "==> 1/4  tuning the release profile for size"
node tools/compact-profile.mjs

echo "==> 2/4  installing frontend dependencies"
if command -v bun >/dev/null 2>&1; then
  bun install --frozen-lockfile
  BUILD_CMD="bun run build"
else
  npm ci --no-audit --no-fund
  BUILD_CMD="npm run build"
fi

echo "==> 3/4  building the frontend"
eval "$BUILD_CMD"

echo "==> 4/4  building the app"
if [[ "$NO_BUNDLE" == "1" ]]; then
  cargo build --release --manifest-path src-tauri/Cargo.toml
  echo
  echo "Binary: src-tauri/target/release/kadr.exe"
  ls -lh src-tauri/target/release/kadr.exe
  exit 0
fi

if command -v bun >/dev/null 2>&1; then
  bunx tauri build --bundles nsis
else
  npx tauri build --bundles nsis
fi

echo
echo "Installer:"
ls -lh src-tauri/target/release/bundle/nsis/*.exe 2>/dev/null || true
echo
echo "Tip: the exe and the installer compress differently — compare with"
echo "     '7z a -mx=9 package.7z src-tauri/target/release/bundle/nsis/*.exe'"
