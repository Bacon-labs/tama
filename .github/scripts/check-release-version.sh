#!/usr/bin/env bash
set -euo pipefail

tag="${1:-${TAMA_RELEASE_TAG:-${GITHUB_REF_NAME:-}}}"
if [ -z "$tag" ]; then
  echo "release tag is required; pass it as argv[1] or set TAMA_RELEASE_TAG/GITHUB_REF_NAME" >&2
  exit 1
fi

case "$tag" in
  v*) ;;
  *)
    echo "release tag must start with v: $tag" >&2
    exit 1
    ;;
esac

expected="${tag#v}"
if [ -z "$expected" ]; then
  echo "release tag is missing a version: $tag" >&2
  exit 1
fi

metadata="$(mktemp)"
trap 'rm -f "$metadata"' EXIT

cargo metadata --locked --format-version=1 --no-deps > "$metadata"
python3 - "$expected" "$metadata" <<'PY'
import json
import sys

expected = sys.argv[1]
metadata_path = sys.argv[2]
with open(metadata_path, encoding="utf-8") as file:
    metadata = json.load(file)

workspace_members = set(metadata["workspace_members"])
versions = {
    package["name"]: package["version"]
    for package in metadata["packages"]
    if package["id"] in workspace_members
}
bad = {
    name: version
    for name, version in sorted(versions.items())
    if version != expected
}
if bad:
    lines = [f"{name}={version}" for name, version in bad.items()]
    raise SystemExit(
        "release tag does not match workspace package versions: "
        + ", ".join(lines)
        + f"; expected {expected}"
    )

required = {"tama-cli", "tamaup-cli"}
missing = sorted(required - set(versions))
if missing:
    raise SystemExit(
        "release version check is missing expected packages: " + ", ".join(missing)
    )
PY

cargo build -p tama-cli -p tamaup-cli --bins

tama_version="$(target/debug/tama --version)"
if [ "$tama_version" != "tama $expected" ]; then
  echo "tama --version mismatch: got '$tama_version', expected 'tama $expected'" >&2
  exit 1
fi

tamaup_version="$(target/debug/tamaup --version)"
if [ "$tamaup_version" != "tamaup $expected" ]; then
  echo "tamaup --version mismatch: got '$tamaup_version', expected 'tamaup $expected'" >&2
  exit 1
fi
