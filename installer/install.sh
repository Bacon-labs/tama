#!/bin/sh
set -eu

BASE_URL="${TAMA_BASE_URL:-https://tama.tools}"
TAMAUP_HOME="${TAMAUP_HOME:-$HOME/.tama}"
VERSION="stable"
NO_MODIFY_PATH=0
OFFLINE=0
MANIFEST_FILE=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --yes) ;;
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

TMP_PARENT="${TMPDIR:-/tmp}"
INSTALL_TMPDIR="$(mktemp -d "${TMP_PARENT%/}/tama-install.XXXXXX")"
trap 'rm -rf "$INSTALL_TMPDIR"' EXIT INT TERM

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
  cp "$MANIFEST_FILE" "$INSTALL_TMPDIR/manifest.json"
  cp "$MANIFEST_FILE.minisig" "$INSTALL_TMPDIR/manifest.json.minisig"
else
  fetch "$BASE_URL/manifest.json" "$INSTALL_TMPDIR/manifest.json"
  fetch "$BASE_URL/manifest.json.minisig" "$INSTALL_TMPDIR/manifest.json.minisig"
fi

PUBLIC_KEY="${TAMA_MINISIGN_PUBLIC_KEY:-RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3}"
minisign -Vm "$INSTALL_TMPDIR/manifest.json" -P "$PUBLIC_KEY" -x "$INSTALL_TMPDIR/manifest.json.minisig" >/dev/null

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required by the bootstrap installer to read the release manifest" >&2
  exit 1
fi

python3 - "$INSTALL_TMPDIR/manifest.json" "$PLATFORM" "$VERSION" > "$INSTALL_TMPDIR/artifact.env" <<'PY'
import json
import re
import shlex
import sys

SAFE_VERSION = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9._+-]*[A-Za-z0-9])?$")
SAFE_SHA256 = re.compile(r"^[0-9a-f]{64}$")
SAFE_PLATFORM = re.compile(r"^[A-Za-z0-9._+-]+$")

def require_string(value, label):
    if not isinstance(value, str) or value == "":
        raise SystemExit(f"invalid release manifest field: {label}")
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
        raise SystemExit(f"unsafe control character in release manifest field: {label}")
    return value

def emit_env(name, value):
    print(f"{name}={shlex.quote(value)}")

def reject_unknown_keys(obj, allowed, label):
    if not isinstance(obj, dict):
        raise SystemExit(f"invalid release manifest field: {label}")
    unknown = sorted(set(obj) - set(allowed))
    if unknown:
        raise SystemExit(f"unknown release manifest field in {label}: {unknown[0]}")

manifest_path, platform, version = sys.argv[1:4]
manifest = json.load(open(manifest_path, encoding="utf-8"))
reject_unknown_keys(manifest, {"schema", "stable", "nightly", "version", "artifacts", "releases"}, "manifest")
schema = manifest.get("schema")
if schema is not None and schema != "tama.release-manifest.v1":
    raise SystemExit(f"unsupported release manifest schema: {schema}")
is_cumulative = schema is not None or "stable" in manifest or "nightly" in manifest or "releases" in manifest
if is_cumulative:
    if schema != "tama.release-manifest.v1":
        raise SystemExit("cumulative release manifest must declare schema tama.release-manifest.v1")
    if "stable" not in manifest:
        raise SystemExit("cumulative release manifest is missing stable version")
    if not isinstance(manifest.get("releases"), list) or not manifest["releases"]:
        raise SystemExit("cumulative release manifest must contain releases[]")
    if "version" in manifest or "artifacts" in manifest:
        raise SystemExit("cumulative release manifest must not mix legacy version/artifacts fields")
    if version in ("stable", "nightly"):
        selected = manifest.get(version)
        if selected is None:
            raise SystemExit(f"release manifest is missing {version} version")
    else:
        selected = version
    releases = manifest["releases"]
else:
    if "version" not in manifest:
        raise SystemExit("legacy release manifest is missing version")
    if not isinstance(manifest.get("artifacts"), list) or not manifest["artifacts"]:
        raise SystemExit("legacy release manifest is missing artifacts")
    selected = manifest["version"] if version == "stable" else version
    releases = [{"version": manifest["version"], "artifacts": manifest["artifacts"]}]
if not SAFE_VERSION.fullmatch(version) or ".." in version:
    raise SystemExit(f"unsafe requested release version: {version}")
selected = require_string(selected, "selected version")
if not SAFE_VERSION.fullmatch(selected) or ".." in selected:
    raise SystemExit(f"unsafe release version: {selected}")
for release in releases:
    reject_unknown_keys(release, {"version", "artifacts"}, "release")
    release_version = require_string(release.get("version"), "release.version")
    release_artifacts = release.get("artifacts")
    if not isinstance(release_artifacts, list) or not release_artifacts:
        raise SystemExit(f"release {release_version} is missing artifacts")
    if not SAFE_VERSION.fullmatch(release_version) or ".." in release_version:
        raise SystemExit(f"unsafe release version: {release_version}")
    for artifact in release_artifacts:
        reject_unknown_keys(artifact, {"platform", "url", "sha256"}, "artifact")
        artifact_platform = require_string(artifact.get("platform"), "artifact.platform")
        artifact_url = require_string(artifact.get("url"), "artifact.url")
        artifact_sha256 = require_string(artifact.get("sha256"), "artifact.sha256")
        if not SAFE_PLATFORM.fullmatch(artifact_platform):
            raise SystemExit(f"unsafe artifact platform: {artifact_platform}")
        if not (artifact_url.startswith("https://") or artifact_url.startswith("file://")):
            raise SystemExit(f"unsupported artifact URL: {artifact_url}")
        if not SAFE_SHA256.fullmatch(artifact_sha256):
            raise SystemExit(f"invalid artifact SHA-256 for {artifact_platform} {release_version}")
        if release_version == selected and artifact_platform == platform:
            emit_env("VERSION", release_version)
            emit_env("URL", artifact_url)
            emit_env("SHA256", artifact_sha256)
            raise SystemExit(0)
raise SystemExit(f"no artifact for {platform} {version}")
PY
. "$INSTALL_TMPDIR/artifact.env"

case "$VERSION" in
  ""|*[!A-Za-z0-9._+-]*|.*|*.) echo "unsafe release version: $VERSION" >&2; exit 1 ;;
  *..*) echo "unsafe release version: $VERSION" >&2; exit 1 ;;
esac

case "$URL" in
  file://*) cp "${URL#file://}" "$INSTALL_TMPDIR/tama.tar.gz" ;;
  *)
    if [ "$OFFLINE" -eq 1 ]; then
      echo "offline install cannot download artifact" >&2
      exit 1
    fi
    fetch "$URL" "$INSTALL_TMPDIR/tama.tar.gz"
    ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "$INSTALL_TMPDIR/tama.tar.gz" | awk '{print $1}')"
else
  ACTUAL="$(shasum -a 256 "$INSTALL_TMPDIR/tama.tar.gz" | awk '{print $1}')"
fi

if [ "$ACTUAL" != "$SHA256" ]; then
  echo "bad SHA-256 for Tama artifact" >&2
  exit 1
fi

tar -tzf "$INSTALL_TMPDIR/tama.tar.gz" > "$INSTALL_TMPDIR/archive.entries"
TAMA_ENTRY_COUNT=0
TAMAUP_ENTRY_COUNT=0
while IFS= read -r entry; do
  case "$entry" in
    /*|*../*|../*) echo "unsafe archive path: $entry" >&2; exit 1 ;;
    bin/tama|tama) TAMA_ENTRY_COUNT=$((TAMA_ENTRY_COUNT + 1)) ;;
    bin/tamaup|tamaup) TAMAUP_ENTRY_COUNT=$((TAMAUP_ENTRY_COUNT + 1)) ;;
    *) echo "unexpected archive entry: $entry" >&2; exit 1 ;;
  esac
done < "$INSTALL_TMPDIR/archive.entries"
if [ "$TAMA_ENTRY_COUNT" -ne 1 ] || [ "$TAMAUP_ENTRY_COUNT" -ne 1 ]; then
  echo "archive must contain exactly one tama and one tamaup binary" >&2
  exit 1
fi

ARCHIVE_STAGE="$INSTALL_TMPDIR/archive"
mkdir -p "$ARCHIVE_STAGE"
tar -xzf "$INSTALL_TMPDIR/tama.tar.gz" -C "$ARCHIVE_STAGE"

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
