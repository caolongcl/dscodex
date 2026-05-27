#!/usr/bin/env bash
# Render Moon Bridge's own Codex config.toml + models_catalog.json into
# $CODEX_HOME (defaults to ~/.codex). Backs up any existing config.toml.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
UPSTREAM_DIR="$HERE/upstream"
CONFIG="$HERE/config.yml"
CODEX_HOME_DIR="${CODEX_HOME:-$HOME/.codex}"

if [ ! -d "$UPSTREAM_DIR/.git" ]; then
  echo "ERROR: upstream/ is missing. Run $HERE/setup.sh first." >&2
  exit 1
fi

if [ ! -f "$CONFIG" ]; then
  echo "ERROR: $CONFIG not found. Run $HERE/setup.sh, then edit it to add your DeepSeek API key." >&2
  exit 1
fi

mkdir -p "$CODEX_HOME_DIR"

if [ -f "$CODEX_HOME_DIR/config.toml" ]; then
  BACKUP="$CODEX_HOME_DIR/config.toml.bak.$(date +%s)"
  cp "$CODEX_HOME_DIR/config.toml" "$BACKUP"
  echo "==> Backed up existing config.toml to $BACKUP"
fi

export GOTOOLCHAIN="${GOTOOLCHAIN:-auto}"
cd "$UPSTREAM_DIR"

MODEL="$(go run ./cmd/moonbridge --config "$CONFIG" --print-codex-model)"
echo "==> Default Codex model alias from Moon Bridge: $MODEL"

go run ./cmd/moonbridge \
  --config "$CONFIG" \
  --print-codex-config "$MODEL" \
  --codex-base-url "http://127.0.0.1:38440/v1" \
  --codex-home "$CODEX_HOME_DIR" \
  > "$CODEX_HOME_DIR/config.toml"

echo "==> Wrote $CODEX_HOME_DIR/config.toml (and models_catalog.json)"
echo
echo "Next: start the proxy in another terminal:"
echo "  $HERE/run.sh"
echo "Then run codex from your project."
