#!/usr/bin/env bash
# Builds the GDExtension (debug, the one the Godot editor binary loads) and launches the game.
# Usage: ./run.sh [--editor] [godot args...]   (GODOT=/path/to/godot to override the binary)
set -euo pipefail
cd "$(dirname "$0")"

editor=()
args=()
for a in "$@"; do
  case "$a" in
    --editor) editor=(--editor) ;;
    *) args+=("$a") ;;
  esac
done

cargo build -p driving_the_earth_ext
exec "${GODOT:-godot}" --path godot "${editor[@]}" "${args[@]}"
