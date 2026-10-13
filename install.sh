#!/bin/sh
# Build DisplayCraft and install it for the current user (or under PREFIX):
# the program, its icons and its launcher entry.
#
#   ./install.sh              install to ~/.local
#   PREFIX=/usr/local ./install.sh
#   ./install.sh --uninstall
set -eu

PREFIX=${PREFIX:-$HOME/.local}
cd "$(dirname "$0")"

bin=$PREFIX/bin/displaycraft
apps=$PREFIX/share/applications
icons=$PREFIX/share/icons/hicolor

refresh() {
    # So the launcher and the window's icon pick up the change without a
    # new login; each tool is optional
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$apps" || true
    command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin" "$apps/displaycraft.desktop"
    for f in "$icons"/*/apps/displaycraft.png; do
        [ -e "$f" ] && rm -f "$f"
    done
    refresh
    echo "Removed DisplayCraft from $PREFIX"
    exit 0
fi

cargo build --release
install -Dm755 target/release/displaycraft "$bin"
for f in assets/icons/hicolor/*/apps/displaycraft.png; do
    install -Dm644 "$f" "$icons/${f#assets/icons/hicolor/}"
done
# An absolute Exec, as the desktop session's PATH may not include $PREFIX/bin
mkdir -p "$apps"
sed "s|^Exec=.*|Exec=$bin|" assets/displaycraft.desktop > "$apps/displaycraft.desktop"
refresh
echo "Installed DisplayCraft to $bin"
