#!/bin/sh
set -eu

BASE_URL="${TAMA_BASE_URL:-https://tama.tools}"
TAMAUP_HOME="${TAMAUP_HOME:-$HOME/.tama}"
VERSION="stable"
YES=0
NO_MODIFY_PATH=0
OFFLINE=0
MANIFEST_FILE=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --yes) YES=1 ;;
    --no-modify-path) NO_MODIFY_PATH=1 ;;
    --offline) OFFLINE=1 ;;
    --version) shift; VERSION="${1:?missing version}" ;;
    --manifest-file) shift; MANIFEST_FILE="${1:?missing manifest file}" ;;
    --no-install-lean|--no-install-foundry|--no-install-solc) ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

case "$(uname -s):$(uname -m)" in
  Linux:x86_64) PLATFORM="linux-x86_64" ;;
  Linux:aarch64|Linux:arm64) PLATFORM="linux-aarch64" ;;
  Darwin:x86_64) PLATFORM="macos-x86_64" ;;
  Darwin:arm64|Darwin:aarch64) PLATFORM="macos-aarch64" ;;
  *) echo "unsupported platform for Tama v0.1" >&2; exit 1 ;;
esac

if ! command -v minisign >/dev/null 2>&1; then
  echo "minisign is required to verify the Tama release manifest" >&2
  echo "Install minisign and rerun this installer, or use tamaup with a verified local manifest." >&2
  exit 1
fi

if [ "$OFFLINE" -eq 1 ] && [ -z "$MANIFEST_FILE" ]; then
  echo "--offline requires --manifest-file" >&2
  exit 1
fi

TMPDIR="${TMPDIR:-/tmp}/tama-install.$$"
mkdir -p "$TMPDIR"
trap 'rm -rf "$TMPDIR"' EXIT INT TERM

fetch() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$out"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$out"
  else
    echo "curl or wget is required" >&2
    exit 1
  fi
}

if [ -n "$MANIFEST_FILE" ]; then
  cp "$MANIFEST_FILE" "$TMPDIR/manifest.json"
  cp "$MANIFEST_FILE.minisig" "$TMPDIR/manifest.json.minisig"
else
  fetch "$BASE_URL/manifest.json" "$TMPDIR/manifest.json"
  fetch "$BASE_URL/manifest.json.minisig" "$TMPDIR/manifest.json.minisig"
fi

PUBLIC_KEY="${TAMA_MINISIGN_PUBLIC_KEY:-RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3}"
minisign -Vm "$TMPDIR/manifest.json" -P "$PUBLIC_KEY" -x "$TMPDIR/manifest.json.minisig" >/dev/null

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required by the bootstrap installer to read the release manifest" >&2
  exit 1
fi

python3 - "$TMPDIR/manifest.json" "$PLATFORM" "$VERSION" > "$TMPDIR/artifact.env" <<'PY'
import json
import sys

manifest_path, platform, version = sys.argv[1:4]
manifest = json.load(open(manifest_path, encoding="utf-8"))
for artifact in manifest["artifacts"]:
    if artifact["platform"] == platform and (version == "stable" or manifest["version"] == version):
        print("VERSION=" + manifest["version"])
        print("URL=" + artifact["url"])
        print("SHA256=" + artifact["sha256"])
        break
else:
    raise SystemExit(f"no artifact for {platform} {version}")
PY
. "$TMPDIR/artifact.env"

case "$VERSION" in
  ""|*[!A-Za-z0-9._+-]*) echo "unsafe release version: $VERSION" >&2; exit 1 ;;
esac

case "$URL" in
  file://*) cp "${URL#file://}" "$TMPDIR/tama.tar.gz" ;;
  *) fetch "$URL" "$TMPDIR/tama.tar.gz" ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "$TMPDIR/tama.tar.gz" | awk '{print $1}')"
else
  ACTUAL="$(shasum -a 256 "$TMPDIR/tama.tar.gz" | awk '{print $1}')"
fi

if [ "$ACTUAL" != "$SHA256" ]; then
  echo "bad SHA-256 for Tama artifact" >&2
  exit 1
fi

tar -tzf "$TMPDIR/tama.tar.gz" > "$TMPDIR/archive.entries"
while IFS= read -r entry; do
  case "$entry" in
    /*|*../*|../*) echo "unsafe archive path: $entry" >&2; exit 1 ;;
    bin/tama|bin/tamaup|tama|tamaup) ;;
    *) echo "unexpected archive entry: $entry" >&2; exit 1 ;;
  esac
done < "$TMPDIR/archive.entries"

ARCHIVE_STAGE="$TMPDIR/archive"
mkdir -p "$ARCHIVE_STAGE"
tar -xzf "$TMPDIR/tama.tar.gz" -C "$ARCHIVE_STAGE"

BAD_ENTRY="$(find "$ARCHIVE_STAGE" ! -type d ! -type f -print | sed -n '1p')"
if [ -n "$BAD_ENTRY" ]; then
  echo "unexpected archive entry type: $BAD_ENTRY" >&2
  exit 1
fi

if [ -f "$ARCHIVE_STAGE/bin/tama" ] && [ -f "$ARCHIVE_STAGE/bin/tamaup" ]; then
  TAMA_BIN="$ARCHIVE_STAGE/bin/tama"
  TAMAUP_BIN="$ARCHIVE_STAGE/bin/tamaup"
elif [ -f "$ARCHIVE_STAGE/tama" ] && [ -f "$ARCHIVE_STAGE/tamaup" ]; then
  TAMA_BIN="$ARCHIVE_STAGE/tama"
  TAMAUP_BIN="$ARCHIVE_STAGE/tamaup"
else
  echo "archive is missing expected tama or tamaup binary" >&2
  exit 1
fi

for binary in "$TAMA_BIN" "$TAMAUP_BIN"; do
  if [ -L "$binary" ]; then
    echo "archive binary must be a regular file: $binary" >&2
    exit 1
  fi
done

VERSIONS_DIR="$TAMAUP_HOME/versions"
VERSION_DIR="$VERSIONS_DIR/$VERSION"
VERSION_TMP="$VERSIONS_DIR/.install-$VERSION.$$"
VERSION_OLD="$VERSIONS_DIR/.previous-$VERSION.$$"
rm -rf "$VERSION_TMP" "$VERSION_OLD"
mkdir -p "$VERSION_TMP/bin"
cp "$TAMA_BIN" "$VERSION_TMP/bin/tama"
cp "$TAMAUP_BIN" "$VERSION_TMP/bin/tamaup"
chmod 755 "$VERSION_TMP/bin/tama" "$VERSION_TMP/bin/tamaup"
if [ -e "$VERSION_DIR" ]; then
  mv "$VERSION_DIR" "$VERSION_OLD"
fi
if mv "$VERSION_TMP" "$VERSION_DIR"; then
  rm -rf "$VERSION_OLD"
else
  if [ -e "$VERSION_OLD" ]; then
    mv "$VERSION_OLD" "$VERSION_DIR"
  fi
  exit 1
fi

mkdir -p "$TAMAUP_HOME/bin"
link_tmp="$TAMAUP_HOME/bin/tama.tmp.$$"
rm -f "$link_tmp"
ln -s "$VERSION_DIR/bin/tama" "$link_tmp"
mv -f "$link_tmp" "$TAMAUP_HOME/bin/tama"
link_tmp="$TAMAUP_HOME/bin/tamaup.tmp.$$"
rm -f "$link_tmp"
ln -s "$VERSION_DIR/bin/tamaup" "$link_tmp"
mv -f "$link_tmp" "$TAMAUP_HOME/bin/tamaup"
printf '%s\n' "$VERSION" > "$TAMAUP_HOME/active.tmp.$$"
mv -f "$TAMAUP_HOME/active.tmp.$$" "$TAMAUP_HOME/active"

if [ "$NO_MODIFY_PATH" -eq 0 ]; then
  echo "Add $TAMAUP_HOME/bin to PATH if it is not already present."
fi

echo "Tama $VERSION installed for $PLATFORM"
