#!/bin/bash
# Install the Omarchy shell plugins: copy shell/ (the background service:
# refresh and meeting alerts) and shell/window/ (the window) into the plugins
# directory (copies, not symlinks: the shell's file watcher doesn't follow
# symlinks, and `omarchy plugin validate` rejects them), then enable them the
# first time. Re-run after editing; saving under the
# plugins directory hot-reloads the shell.
set -euo pipefail

src="$(cd "$(dirname "$0")" && pwd)"
plugins="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins"

# copy <plugin dir> <id> <files...>
copy() {
  local from="$1" id="$2"; shift 2
  local dest="$plugins/$id"
  mkdir -p "$dest"
  for f in "$@"; do install -m 644 "$from/$f" "$dest/$f"; done
  omarchy plugin validate "$dest"
}

# The service (refresh, alerts) and the window
copy "$src" local.calendarchy manifest.json Service.qml Model.js
copy "$src/window" local.calendarchy.window manifest.json Window.qml Layout.js

# The file watcher reloads after a debounce; rescan now so enable can see them
omarchy-shell shell rescanPlugins >/dev/null

enabled() { omarchy plugin list --json | jq -e --arg id "$1" 'any(.[]; .id == $id and .enabled)' >/dev/null; }
enabled local.calendarchy || omarchy plugin enable local.calendarchy
enabled local.calendarchy.window || omarchy plugin enable local.calendarchy.window
echo "installed local.calendarchy and local.calendarchy.window → $plugins"
