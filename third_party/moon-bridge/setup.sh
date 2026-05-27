#!/usr/bin/env bash
# Idempotently clone (or update) Moon Bridge into upstream/ and bootstrap
# a local config.yml from the example. Re-run any time to fast-forward
# the upstream checkout.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
UPSTREAM_DIR="$HERE/upstream"
UPSTREAM_REPO="https://github.com/ZhiYi-R/moon-bridge.git"

if [ ! -d "$UPSTREAM_DIR/.git" ]; then
  echo "==> Cloning Moon Bridge into $UPSTREAM_DIR"
  git clone --depth 1 "$UPSTREAM_REPO" "$UPSTREAM_DIR"
else
  echo "==> Updating Moon Bridge in $UPSTREAM_DIR"
  git -C "$UPSTREAM_DIR" fetch --depth 1 origin
  git -C "$UPSTREAM_DIR" reset --hard FETCH_HEAD
fi

CONFIG="$HERE/config.yml"
EXAMPLE="$HERE/config.example.yml"
if [ ! -f "$CONFIG" ]; then
  cp "$EXAMPLE" "$CONFIG"
  echo "==> Wrote $CONFIG (copy of config.example.yml)"
  echo "    Edit it and replace sk-REPLACE-WITH-YOUR-DEEPSEEK-API-KEY with your real key."
else
  echo "==> $CONFIG already exists; leaving it untouched."
fi

# Go version check — Moon Bridge requires 1.25+.
if command -v go >/dev/null 2>&1; then
  GO_VERSION="$(go version | awk '{print $3}' | sed 's/^go//')"
  case "$GO_VERSION" in
    1.2[5-9]*|1.[3-9][0-9]*|[2-9].*)
      echo "==> Detected go $GO_VERSION (OK)"
      ;;
    *)
      echo "WARNING: go $GO_VERSION detected; Moon Bridge requires 1.25+." >&2
      echo "         GOTOOLCHAIN=auto (Go 1.21+ default) will auto-download 1.25 on first run." >&2
      ;;
  esac
else
  echo "ERROR: go is not in PATH. Install Go 1.25+ from https://go.dev/dl/" >&2
  exit 1
fi

echo
echo "Next:"
echo "  1) Edit $CONFIG and set api_key"
echo "  2) Run $HERE/run.sh"
