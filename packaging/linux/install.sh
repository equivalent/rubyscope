#!/usr/bin/env bash
# Installs Rubyscope from this release folder for this user:
#   ~/.local/bin/rubyscope and ~/.local/bin/rubyscope-cli
#   ~/.local/share/applications/rubyscope.desktop (the app launcher entry, and
#   the handler of rubyscope:// links)
#   ~/.local/share/icons/hicolor/*/apps/rubyscope.{svg,png} (the app icon)
set -euo pipefail
cd "$(dirname "$0")"
install -Dm755 rubyscope "$HOME/.local/bin/rubyscope"
install -Dm755 rubyscope-cli "$HOME/.local/bin/rubyscope-cli"
install -Dm644 rubyscope.desktop "$HOME/.local/share/applications/rubyscope.desktop"
icons="$HOME/.local/share/icons/hicolor"
install -Dm644 icons/rubyscope.svg "$icons/scalable/apps/rubyscope.svg"
for size in 16 32 48 64 128 256 512; do
  install -Dm644 "icons/rubyscope-$size.png" "$icons/${size}x${size}/apps/rubyscope.png"
done
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$icons" || true
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
# rubyscope://open?... links open in Rubyscope.
command -v xdg-mime >/dev/null && xdg-mime default rubyscope.desktop x-scheme-handler/rubyscope || true
echo "Installed Rubyscope. Run: rubyscope <project folder>"
