#!/usr/bin/env bash
# Launch Moon Bridge with the local config.yml. Forwards extra args to moonbridge.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
UPSTREAM_DIR="$HERE/upstream"
CONFIG="$HERE/config.yml"

if [ ! -d "$UPSTREAM_DIR/.git" ]; then
  echo "ERROR: upstream/ is missing. Run $HERE/setup.sh first." >&2
  exit 1
fi

if [ ! -f "$CONFIG" ]; then
  echo "ERROR: $CONFIG not found. Run $HERE/setup.sh, then edit it to add your DeepSeek API key." >&2
  exit 1
fi

if grep -F 'sk-REPLACE-WITH-YOUR-DEEPSEEK-API-KEY' "$CONFIG" >/dev/null 2>&1; then
  echo "ERROR: $CONFIG still has the placeholder api_key." >&2
  echo "       Edit it and replace sk-REPLACE-WITH-YOUR-DEEPSEEK-API-KEY." >&2
  exit 1
fi

# GOTOOLCHAIN=auto (Go 1.21+ default) auto-fetches the toolchain pinned in go.mod.
export GOTOOLCHAIN="${GOTOOLCHAIN:-auto}"

cd "$UPSTREAM_DIR"
echo "==> Starting Moon Bridge with $CONFIG"
exec go run ./cmd/moonbridge --config "$CONFIG" "$@"
