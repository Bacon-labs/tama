#!/bin/sh
set -eu

BASE_URL="https://github.com/bacon-labs/tama/releases/latest/download"
TAMAUP_HOME="${TAMAUP_HOME:-$HOME/.tama}"

if [ "$#" -gt 0 ]; then
  echo "install.sh takes no arguments" >&2
  exit 2
fi

case "$(uname -s):$(uname -m)" in
  Linux:x86_64) PLATFORM="linux-x86_64" ;;
  Linux:aarch64|Linux:arm64) PLATFORM="linux-aarch64" ;;
  Darwin:arm64|Darwin:aarch64) PLATFORM="macos-aarch64" ;;
  *) echo "unsupported platform: $(uname -s):$(uname -m)" >&2; exit 1 ;;
esac

TMP_PARENT="${TMPDIR:-/tmp}"
INSTALL_TMPDIR="$(mktemp -d "${TMP_PARENT%/}/tama-install.XXXXXX")"
trap 'rm -rf "$INSTALL_TMPDIR"' EXIT INT TERM

fetch() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 --retry-delay 2 --retry-connrefused "$url" -o "$out"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --tries=3 --waitretry=2 "$url" -O "$out"
  else
    echo "curl or wget is required" >&2
    exit 1
  fi
}

fetch "$BASE_URL/manifest.json" "$INSTALL_TMPDIR/manifest.json"

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required by the bootstrap installer to read the release manifest" >&2
  exit 1
fi

python3 - "$INSTALL_TMPDIR/manifest.json" "$PLATFORM" > "$INSTALL_TMPDIR/artifact.env" <<'PY'
import json
import re
import shlex
import sys

SAFE_VERSION = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9._+-]*[A-Za-z0-9])?$")
SAFE_SHA256 = re.compile(r"^[0-9a-f]{64}$")
SAFE_PLATFORM = re.compile(r"^[A-Za-z0-9._+-]+$")
HTTPS_URL = re.compile(r"^https://[A-Za-z0-9]")

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

manifest_path, platform = sys.argv[1:3]
manifest = json.load(open(manifest_path, encoding="utf-8"))
reject_unknown_keys(manifest, {"schema", "stable", "nightly", "releases"}, "manifest")
schema = manifest.get("schema")
if schema != "tama.release-manifest.v1":
    raise SystemExit(f"unsupported release manifest schema: {schema}")
if "stable" not in manifest:
    raise SystemExit("release manifest is missing stable version")
if not isinstance(manifest.get("releases"), list) or not manifest["releases"]:
    raise SystemExit("release manifest must contain releases[]")
selected = require_string(manifest.get("stable"), "stable")
if not SAFE_VERSION.fullmatch(selected) or ".." in selected:
    raise SystemExit(f"unsafe release version: {selected}")
release_versions = set()
selected_artifact = None
for release in manifest["releases"]:
    reject_unknown_keys(release, {"version", "artifacts"}, "release")
    release_version = require_string(release.get("version"), "release.version")
    release_artifacts = release.get("artifacts")
    if not isinstance(release_artifacts, list) or not release_artifacts:
        raise SystemExit(f"release {release_version} is missing artifacts")
    if not SAFE_VERSION.fullmatch(release_version) or ".." in release_version:
        raise SystemExit(f"unsafe release version: {release_version}")
    if release_version in release_versions:
        raise SystemExit(f"duplicate release version in release manifest: {release_version}")
    release_versions.add(release_version)
    artifact_platforms = set()
    for artifact in release_artifacts:
        reject_unknown_keys(artifact, {"platform", "url", "sha256"}, "artifact")
        artifact_platform = require_string(artifact.get("platform"), "artifact.platform")
        artifact_url = require_string(artifact.get("url"), "artifact.url")
        artifact_sha256 = require_string(artifact.get("sha256"), "artifact.sha256")
        if not SAFE_PLATFORM.fullmatch(artifact_platform):
            raise SystemExit(f"unsafe artifact platform: {artifact_platform}")
        if artifact_platform in artifact_platforms:
            raise SystemExit(f"duplicate artifact platform in release manifest: {artifact_platform}")
        artifact_platforms.add(artifact_platform)
        if not artifact_url.startswith("https://") or not HTTPS_URL.match(artifact_url):
            raise SystemExit(f"artifact URL must use https:// with a host: {artifact_url}")
        if not SAFE_SHA256.fullmatch(artifact_sha256):
            raise SystemExit(f"invalid artifact SHA-256 for {artifact_platform} {release_version}")
        if release_version == selected and artifact_platform == platform:
            selected_artifact = (release_version, artifact_url, artifact_sha256)
if selected not in release_versions:
    raise SystemExit(f"release manifest stable channel points to unknown release: {selected}")
if selected_artifact is None:
    raise SystemExit(f"no artifact for {platform} in release {selected}")
emit_env("VERSION", selected_artifact[0])
emit_env("URL", selected_artifact[1])
emit_env("SHA256", selected_artifact[2])
PY
# shellcheck source=/dev/null
. "$INSTALL_TMPDIR/artifact.env"

case "$VERSION" in
  ""|*[!A-Za-z0-9._+-]*|.*|*.) echo "unsafe release version: $VERSION" >&2; exit 1 ;;
  *..*) echo "unsafe release version: $VERSION" >&2; exit 1 ;;
esac

fetch "$URL" "$INSTALL_TMPDIR/tama.tar.gz"

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
    /*|*../*) echo "unsafe archive path: $entry" >&2; exit 1 ;;
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

sh_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

fish_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e "s/'/\\\\'/g")"
}

QUOTED_BIN_SH="$(sh_quote "$TAMAUP_HOME/bin")"
ENV_TMP="$TAMAUP_HOME/.env.tmp.$$"
rm -f "$ENV_TMP"
cat > "$ENV_TMP" <<EOF
#!/bin/sh
# tama shell setup; generated by install.sh
case ":\${PATH}:" in
  *:$QUOTED_BIN_SH:*) ;;
  *) PATH=$QUOTED_BIN_SH"\${PATH:+:\$PATH}"; export PATH ;;
esac
EOF
chmod 644 "$ENV_TMP"
mv -f "$ENV_TMP" "$TAMAUP_HOME/env"

QUOTED_BIN_FISH="$(fish_quote "$TAMAUP_HOME/bin")"
ENV_FISH_TMP="$TAMAUP_HOME/.env.fish.tmp.$$"
rm -f "$ENV_FISH_TMP"
cat > "$ENV_FISH_TMP" <<EOF
# tama shell setup; generated by install.sh
if not contains -- $QUOTED_BIN_FISH \$PATH
    set -gx PATH $QUOTED_BIN_FISH \$PATH
end
EOF
chmod 644 "$ENV_FISH_TMP"
mv -f "$ENV_FISH_TMP" "$TAMAUP_HOME/env.fish"

QUOTED_ENV_SH="$(sh_quote "$TAMAUP_HOME/env")"
SOURCE_LINE_SH=". $QUOTED_ENV_SH"

append_source_line() {
  profile="$1"
  [ -f "$profile" ] || return 1
  if grep -Fqx "$SOURCE_LINE_SH" "$profile" 2>/dev/null; then
    return 0
  fi
  if printf '\n%s\n' "$SOURCE_LINE_SH" >> "$profile" 2>/dev/null; then
    return 0
  fi
  echo "warning: could not update $profile; add the following line manually:" >&2
  echo "  $SOURCE_LINE_SH" >&2
  return 1
}

PROFILES_TOUCHED=0
for profile in "$HOME/.profile" "$HOME/.bash_profile" "$HOME/.bashrc" "$HOME/.zshenv" "$HOME/.zshrc"; do
  if append_source_line "$profile"; then
    PROFILES_TOUCHED=$((PROFILES_TOUCHED + 1))
  fi
done
if [ "$PROFILES_TOUCHED" -eq 0 ]; then
  if : >> "$HOME/.profile" 2>/dev/null; then
    append_source_line "$HOME/.profile" || true
  fi
fi

FISH_CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d"
case "${SHELL:-}" in
  */fish) mkdir -p "$FISH_CONF_DIR" 2>/dev/null || true ;;
esac
if [ -d "$FISH_CONF_DIR" ]; then
  QUOTED_ENV_FISH="$(fish_quote "$TAMAUP_HOME/env.fish")"
  fish_snippet="$FISH_CONF_DIR/tama.fish"
  fish_tmp="$FISH_CONF_DIR/.tama.fish.tmp.$$"
  rm -f "$fish_tmp"
  if {
    printf '# tama shell setup; generated by install.sh\n'
    printf 'source %s\n' "$QUOTED_ENV_FISH"
  } > "$fish_tmp" 2>/dev/null; then
    mv -f "$fish_tmp" "$fish_snippet"
  else
    rm -f "$fish_tmp"
  fi
fi

echo "Tama $VERSION installed for $PLATFORM"
case ":$PATH:" in
  *":$TAMAUP_HOME/bin:"*) ;;
  *)
    echo "Added $TAMAUP_HOME/bin to PATH for new shells."
    echo "To use tama in this shell, run:"
    echo "  . $QUOTED_ENV_SH"
    ;;
esac
