# Building Kadr locally (Windows)

Everything here runs on your machine. No cloud, no telemetry, no artifact
downloads from anyone else's repo.

## Build in the cloud (no toolchain on your machine)

Push the code to GitHub and let a Windows runner build the installer:

```powershell
powershell -ExecutionPolicy Bypass -File tools\publish.ps1
```

Then Actions -> "Windows installer" -> Run workflow. The artifact appears in
~20 minutes. Every push to `master` rebuilds it; a `v*` tag publishes a Release
with the exe attached. Workflow file:
[`.github/workflows/windows-installer.yml`](../.github/workflows/windows-installer.yml).

The workflow runs `tsc --noEmit` and `vitest run` before the Rust build, so a
typo fails in two minutes instead of twenty, and it takes a `slim` input for the
small installer. Upstream's own release workflow was removed from this fork: it
signs updater artifacts with `TAURI_SIGNING_PRIVATE_KEY`, which a personal fork
does not have.

Only the release step needs write permission for the Actions token. If it fails
with 403: Settings -> Actions -> General -> Workflow permissions -> "Read and
write permissions".

## Fastest local path (one command)

```powershell
# PowerShell as administrator, from the source folder
powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1
```

Checks for Rust, the MSVC build tools, Node and WebView2, installs what is
missing via winget, then builds the installer. Flags: `-Dev` (run with hot
reload instead), `-Slim` (small installer), `-SkipPrereqs` (build only).
A Russian walkthrough lives in [QUICKSTART.ru.md](QUICKSTART.ru.md).

## One-time setup (manual)

1. **Rust** — <https://rustup.rs> (`x86_64-pc-windows-msvc` toolchain, the
   default on Windows).
2. **Visual Studio Build Tools 2022** — the *Desktop development with C++*
   workload. The MSVC linker is not optional; Rust on Windows needs it.
3. **Node.js 20+** (or Bun, which is faster: <https://bun.sh>).
4. **WebView2** — already installed on Windows 10 (2021+) and Windows 11. On
   older machines, install the Evergreen Runtime once.

Verify:

```powershell
rustc --version      # 1.8x+
cargo --version
node --version       # or: bun --version
```

## Build the installer

```powershell
git clone <your fork>
cd kadr
npm ci                 # or: bun install --frozen-lockfile
npm run dist           # → src-tauri\target\release\bundle\nsis\kadr_1.0.16_x64-setup.exe
```

`npm run dist` does three things: tunes the release profile for size
(`tools/compact-profile.mjs`), builds the frontend, and runs `tauri build
--bundles nsis`.

Other targets:

```powershell
npm run dist:dir       # just kadr.exe, no installer
npx tauri build --bundles msi     # MSI instead of NSIS
npx tauri dev          # dev build with hot reload
```

Cold build takes ~10–20 minutes; incremental rebuilds are seconds.

## How compact can the exe be?

Measured on this fork (Linux x86-64, release profile with `strip` +
`panic = "abort"`; a Windows build is within a few hundred KB of these numbers,
and the installer adds ~2 MB of NSIS + uninstaller):

| Build | Binary | Compressed (xz -9, ≈ installer payload) |
|---|---|---|
| Full — PP-OCR/ONNX compiled in (default) | 36.1 MB | 9.4 MB |
| Slim — `default = []` | 25.5 MB | 6.9 MB |

So the answer to "can it be packed compactly" is yes: the thing you download is
a ~7–9 MB installer, and the two levers below are what get it there. The size
that most people quote for Tauri apps (100+ MB) comes from *bundling WebView2*,
which this config deliberately does not do — Windows 10 (2021+) and 11 ship it.



The download is a *compressed* installer, so its size is not the exe's size.
What actually moves the needle, in order of impact:

| Lever | Effect | Where |
|---|---|---|
| `strip = true` + `panic = "abort"` | −3…6 MB (drops debug info already stripped, plus unwind tables) | `[profile.release]` |
| `lto = "thin"` | −1…3 MB over `lto = false`, no measurable slowdown | `[profile.release]` |
| Not bundling WebView2 | −120 MB (WebView2 is a system component on Win10 2021+/11) | `bundle.windows.webviewInstallMode` |
| Dropping `accurate-ocr` | −30…45 MB (the vendored PP-OCR + ONNX Runtime) | `[features] default = []` |
| Dropping OCR entirely (Tesseract only) | −60…80 MB total | remove `tesseract`/OCR deps |
| NSIS vs MSI | NSIS is smaller by ~10–20 % | `--bundles nsis` |
| `opt-level = "s"` (whole binary) | −5…8 %, but 10–20 % slower search | not recommended |

Two pre-built profiles are one command away:

```powershell
# the default build (OCR + PP-OCR compiled in): ~9 MB installer
npm run dist

# the slim build: no PP-OCR / ONNX Runtime: ~7 MB installer
npm run dist:slim

# switch the feature set without building (then build as usual)
npm run size:slim     # default = []
npm run size:full     # default = ["accurate-ocr"]
```

`dist:slim` (which runs `node tools/compact-profile.mjs --slim`) sets `default = []` in `src-tauri/Cargo.toml`;
`--full` puts it back. The trade: the optional "Accurate OCR" engine can no
longer be enabled at runtime, and Tesseract (if installed) becomes the only OCR
engine. Everything else — names, folders, dates, sizes, PDF/Office text, images
by name and metadata — is untouched. For a photo/video library that is usually
the right trade.

Note: the vendored `accurate-ocr` path did not actually compile with
`default = []` until this fork gated `ocr_indra::self_test` behind the feature
(`pool` does not exist without it), so "slim" was documented upstream but not
buildable. It builds now.

## Rebranding

```powershell
tools/rebrand.sh "Kadr" "app.kadr.desktop" "kadr"
```

Renames the product, the bundle identifier and the executable, and sweeps
user-visible strings. The **data directory stays `kensaku`** on purpose: an
existing install keeps its index, so a rebrand does not cost a re-crawl. Rust
crate names are untouched too — nothing user-visible, and the CLI crate links
against them.

Rebuilding with a *new* bundle identifier gives the webview a fresh profile
(`%LOCALAPPDATA%\<identifier>`); settings and index are unaffected.

## Updating from upstream

This fork carries the full-result pagination fix plus the collection skill. To
pull in later upstream work:

```powershell
git remote add upstream https://github.com/yjid/kensaku
git fetch upstream
git merge upstream/master        # conflicts are unlikely outside src/search and App.tsx
npm run dist
```

The fork is a normal git repository — nothing depends on the upstream repo
existing at build time.

## Troubleshooting

- **`linker 'link.exe' not found`** — Visual Studio Build Tools are missing, or
  you are in a shell that has not picked up the environment. Use
  *Developer PowerShell for VS*, or reinstall Build Tools with the C++ workload.
- **`WebView2 runtime not found`** at launch — install the Evergreen Runtime.
- **Antivirus quarantines a unsigned NSIS installer** — expected for an unsigned
  binary. Sign it (`signtool`) or add an exclusion; the code is yours to read.
- **`error: could not compile kadr (lib)` after a rebrand** — run
  `cargo clean -p kensaku` (crate name unchanged on purpose) and rebuild.
