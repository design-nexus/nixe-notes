#!/usr/bin/env bash
set -euo pipefail
# Piped from curl (`curl -fsSL …/install.sh | bash`): no checkout here, so fetch
# the source and run its installer from there.
if [[ -z ${BASH_SOURCE[0]:-} ]]; then
  command -v git >/dev/null || { echo "Git is needed to install nyx-notes." >&2; exit 1; }
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  git clone -q --depth 1 https://github.com/design-nexus/nyx-notes.git "$work/src"
  bash "$work/src/install.sh" "$@"
  exit
fi
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# A release archive (.prebuilt) brings its binaries.
if [[ ${NYX_SKIP_BUILD:-0} != 1 && ! -f $root/.prebuilt ]]; then
  cargo build --release --locked --manifest-path "$root/Cargo.toml" -p nyx-notes -p nyx-notes-glimpse
fi
share="${XDG_DATA_HOME:-$HOME/.local/share}"
binary="${CARGO_TARGET_DIR:-$root/target}/release/nyx-notes"
[[ -f $binary ]] || { echo "Release binary missing: $binary" >&2; exit 1; }
install -Dm755 "$binary" "$HOME/.local/bin/nyx-notes.new"
mv -f "$HOME/.local/bin/nyx-notes.new" "$HOME/.local/bin/nyx-notes"
plugin_binary="${CARGO_TARGET_DIR:-$root/target}/release/nyx-notes-glimpse"
install -Dm755 "$plugin_binary" "$HOME/.local/bin/nyx-notes-glimpse"
install -Dm755 "$plugin_binary" "$share/nyx-glimpse/plugins/nyx-notes/nyx-notes-glimpse"
install -Dm644 "$root/data/glimpse/plugin.toml" "$share/nyx-glimpse/plugins/nyx-notes/plugin.toml"
install -Dm644 "$root/data/io.github.design_nexus.nyx.Notes.desktop" "$share/applications/io.github.design_nexus.nyx.Notes.desktop"
install -Dm644 "$root/data/icons/io.github.design_nexus.nyx.Notes.svg" "$share/icons/hicolor/scalable/apps/io.github.design_nexus.nyx.Notes.svg"
for document in README.md LICENSE docs/SYNC.md docs/EXTENSIONS.md docs/INTEGRATIONS.md; do
  install -Dm644 "$root/$document" "$share/nyx-notes/docs/${document##*/}"
done
if command -v update-desktop-database >/dev/null; then update-desktop-database "$share/applications"; fi
printf 'Installed Notes without opening it.\n'
