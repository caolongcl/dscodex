#!/usr/bin/env bash
# Build the `codex` binary from this checkout and install it so that
# running `codex` from any directory invokes the freshly built binary.
#
# Usage:
#   scripts/build-and-install-local.sh                  # debug build (fast iteration), install to ~/.local/bin
#   scripts/build-and-install-local.sh --release        # release build (slower compile, faster runtime)
#   scripts/build-and-install-local.sh --prefix DIR     # install into DIR (e.g. /usr/local/bin)
#   scripts/build-and-install-local.sh --no-path        # do not edit shell profile
#
# The script copies (not symlinks) the binary so it keeps working even if
# you `cargo clean` afterwards. Re-run the script to refresh.

set -euo pipefail

PROFILE="debug"
PREFIX="${CODEX_INSTALL_DIR:-$HOME/.local/bin}"
EDIT_PATH=1

while [ "$#" -gt 0 ]; do
  case "$1" in
    --debug)
      PROFILE="debug"
      ;;
    --release)
      PROFILE="release"
      ;;
    --prefix)
      [ "$#" -ge 2 ] || { echo "--prefix requires a directory" >&2; exit 1; }
      PREFIX="$2"
      shift
      ;;
    --no-path)
      EDIT_PATH=0
      ;;
    -h|--help)
      sed -n '2,14p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
  shift
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_DIR="$REPO_ROOT/codex-rs"

command -v cargo >/dev/null 2>&1 || {
  echo "cargo not found in PATH. Install Rust via https://rustup.rs first." >&2
  exit 1
}

echo "==> Building codex ($PROFILE) from $CARGO_DIR"
if [ "$PROFILE" = "release" ]; then
  (cd "$CARGO_DIR" && cargo build --release --bin codex)
  BUILT_BIN="$CARGO_DIR/target/release/codex"
else
  (cd "$CARGO_DIR" && cargo build --bin codex)
  BUILT_BIN="$CARGO_DIR/target/debug/codex"
fi

[ -x "$BUILT_BIN" ] || { echo "Build did not produce $BUILT_BIN" >&2; exit 1; }

mkdir -p "$PREFIX"
DEST="$PREFIX/codex"

echo "==> Installing to $DEST"
# Write to a temp file in the same directory then mv, so a running `codex`
# is not clobbered mid-read.
TMP="$DEST.tmp.$$"
cp "$BUILT_BIN" "$TMP"
chmod 0755 "$TMP"
# On macOS, cargo adhoc-signs the binary at its build path. After cp the
# embedded signature no longer matches the new path and the kernel SIGKILLs
# the process on launch. Re-adhoc-sign at the destination.
if [ "$(uname -s)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then
  codesign --sign - --force "$TMP" 2>/dev/null || true
fi
mv -f "$TMP" "$DEST"

if [ "$EDIT_PATH" -eq 1 ]; then
  case ":$PATH:" in
    *":$PREFIX:"*)
      ON_PATH=1
      ;;
    *)
      ON_PATH=0
      ;;
  esac

  if [ "$ON_PATH" -eq 0 ]; then
    case "${SHELL:-}" in
      */zsh)  PROFILE_FILE="$HOME/.zshrc" ;;
      */bash)
        if [ "$(uname -s)" = "Darwin" ]; then
          PROFILE_FILE="$HOME/.bash_profile"
        else
          PROFILE_FILE="$HOME/.bashrc"
        fi
        ;;
      */fish) PROFILE_FILE="$HOME/.config/fish/config.fish" ;;
      *)      PROFILE_FILE="$HOME/.profile" ;;
    esac

    BEGIN_MARKER="# >>> codex local install >>>"
    END_MARKER="# <<< codex local install <<<"
    if [ -f "$PROFILE_FILE" ] && grep -F "$BEGIN_MARKER" "$PROFILE_FILE" >/dev/null 2>&1; then
      echo "==> PATH block already present in $PROFILE_FILE"
    else
      mkdir -p "$(dirname "$PROFILE_FILE")"
      case "$PROFILE_FILE" in
        */config.fish)
          {
            printf '\n%s\n' "$BEGIN_MARKER"
            printf 'set -gx PATH %s $PATH\n' "$PREFIX"
            printf '%s\n' "$END_MARKER"
          } >>"$PROFILE_FILE"
          ;;
        *)
          {
            printf '\n%s\n' "$BEGIN_MARKER"
            printf 'export PATH="%s:$PATH"\n' "$PREFIX"
            printf '%s\n' "$END_MARKER"
          } >>"$PROFILE_FILE"
          ;;
      esac
      echo "==> Added $PREFIX to PATH in $PROFILE_FILE"
      echo "    Open a new shell or run: export PATH=\"$PREFIX:\$PATH\""
    fi
  fi
fi

echo "==> Installed: $("$DEST" --version 2>/dev/null || echo "$DEST")"
echo "    Run \`codex\` from any directory (after reloading PATH if it was just added)."
