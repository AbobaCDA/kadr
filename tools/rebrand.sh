#!/usr/bin/env bash
# Rebrands this fork in one shot.
#
#   tools/rebrand.sh "Kadr" "app.kadr.desktop" "kadr"
#
#   $1  product name shown to users (installer, window, tray, menu bar)
#   $2  bundle identifier (must be reverse-DNS; changing it moves WebView data)
#   $3  executable name (kadr → Kadr.exe). Lowercase, no spaces.
#
# What it does NOT touch, on purpose:
#   - the data directory name ("kensaku": settings, index, metadata DB). Keeping
#     it is what lets an existing install keep its index after a rebrand —
#     re-indexing a 100k-file library to rename a folder is a bad trade.
#   - Rust crate and module paths (`kensaku_lib`), which no user ever sees and
#     which the CLI crate links against.
set -euo pipefail

NAME="${1:-}"
IDENTIFIER="${2:-}"
BIN="${3:-}"

if [[ -z "$NAME" || -z "$IDENTIFIER" || -z "$BIN" ]]; then
  echo "usage: tools/rebrand.sh <Product Name> <reverse.dns.identifier> <exe-name>" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

python3 - "$NAME" "$IDENTIFIER" "$BIN" <<'PY'
import json, pathlib, re, sys

name, identifier, binary = sys.argv[1], sys.argv[2], sys.argv[3]
root = pathlib.Path(".")

# ---------------------------------------------------------------- tauri.conf
conf_path = root / "src-tauri/tauri.conf.json"
conf = json.loads(conf_path.read_text())
conf["productName"] = binary  # Tauri derives Kadr.exe / Kadr.app from this
conf["identifier"] = identifier
bundle = conf["bundle"]
bundle["publisher"] = name
bundle["copyright"] = f"Copyright © {name}"
bundle["shortDescription"] = "Fast local search for images, videos and files."
bundle["longDescription"] = (
    f"{name} indexes filenames, file contents, PDFs, Office documents and "
    "OCR'd images into one local index, and files what you find into a "
    "collection folder with a single key. Nothing leaves the machine."
)
bundle.pop("homepage", None)
conf_path.write_text(json.dumps(conf, indent=2, ensure_ascii=False) + "\n")

# ------------------------------------------------------------------ manifests
pkg_path = root / "package.json"
pkg = json.loads(pkg_path.read_text())
pkg["name"] = binary
pkg_path.write_text(json.dumps(pkg, indent=2, ensure_ascii=False) + "\n")

cargo_path = root / "src-tauri/Cargo.toml"
cargo = cargo_path.read_text()
cargo = re.sub(
    r'^default-run = ".*"$', f'default-run = "{binary}"', cargo, flags=re.M
)
# Rename only the binary, not the package: the lib target and the CLI crate
# depend on the package name.
if "[[bin]]" in cargo:
    cargo = re.sub(r'(?s)\[\[bin\]\].*?(?=\n\[)', f'[[bin]]\nname = "{binary}"\npath = "src/main.rs"\n\n', cargo, count=1)
else:
    cargo = cargo.replace(
        '[[package]]', '[[package]]', 1
    )
    marker = "\n[dependencies]"
    cargo = cargo.replace(
        marker,
        f'\n[[bin]]\nname = "{binary}"\npath = "src/main.rs"\n' + marker,
        1,
    )
cargo_path.write_text(cargo)

# ------------------------------------------------------- user-visible strings
def sweep(pattern: str, skip=()):
    """Replace the old product name in user-visible text, never in code names."""
    for path in root.rglob(pattern):
        if any(part in {".git", "node_modules", "target", "dist"} for part in path.parts):
            continue
        if path.suffix in {".png", ".ico", ".icns", ".jpg", ".lock"}:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        # KensakuConfig / KENSAKU_DATA_DIR / kensaku_lib / kensaku-daemon: code.
        # Bare "Kensaku" in a string or comment: branding.
        updated = re.sub(r"(?<![\w.\-/])Kensaku(?![\w])", name, text)
        updated = updated.replace('"name": "kensaku"', f'"name": "{binary}"')
        if updated != text:
            path.write_text(updated, encoding="utf-8")
            print(f"  rebranded {path.relative_to(root)}")

for pattern in ("*.rs", "*.tsx", "*.ts", "*.json", "*.html", "*.nsh", "*.md"):
    sweep(pattern)
PY

# ---------------------------------------------------------------- binary refs
# The NSIS hooks and portable marker refer to the executable by name.
sed -i "s/kensaku\.exe/${BIN}.exe/g; s/Kensaku\.exe/${BIN}.exe/g" src-tauri/nsis/hooks.nsh 2>/dev/null || true
sed -i "s/\"Kensaku\"/\"${NAME}\"/g" src-tauri/nsis/hooks.nsh 2>/dev/null || true

echo
echo "Rebranded to \"${NAME}\" (${IDENTIFIER}), executable ${BIN}."
echo "Data directory intentionally unchanged: $(grep -o 'join("kensaku")' src-tauri/src/config.rs | head -1)."
echo "Next: npm run dist   (Windows installer)"
