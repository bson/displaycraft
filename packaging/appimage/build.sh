#!/bin/sh
# Build DisplayCraft-<version>-x86_64.AppImage, with the Wayland ArgyllCMS
# inside, in an Ubuntu 24.04 container. Ubuntu 24.04's glibc (2.39) is the
# oldest on a distribution with Plasma 6.4 or later, which the Wayland
# ArgyllCMS needs, so the AppImage runs wherever that does. The steps are
# make-appimage.sh's, which GitHub Actions runs directly.
#
#   git submodule update --init
#   packaging/appimage/build.sh
#
# Settings, from the environment:
#   ARGYLL_SRC   the Wayland ArgyllCMS (default: the argyllcms submodule)
#   OUT          where the AppImage goes (default: dist/)
#   ENGINE       podman (default) or docker
#
# The container downloads Ubuntu packages, rustup and Rust crates, and
# appimagetool from the AppImage project's GitHub releases. They're kept in
# tmp/appimage-cache, so later builds are quicker.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
ARGYLL_SRC=$(cd "${ARGYLL_SRC:-$root/argyllcms}" && pwd)
OUT=$(mkdir -p "${OUT:-$root/dist}" && cd "${OUT:-$root/dist}" && pwd)
ENGINE=${ENGINE:-podman}
CACHE=$root/tmp/appimage-cache
mkdir -p "$CACHE"

[ -e "$ARGYLL_SRC/Jamtop" ] || { echo "No ArgyllCMS in $ARGYLL_SRC: run git submodule update --init" >&2; exit 1; }

# A submodule's git data is in this repository's .git, so it's reached
# through this repository's mount; a separate checkout gets its own
case "$ARGYLL_SRC/" in
    "$root"/*) argyll_in=/src/displaycraft/${ARGYLL_SRC#"$root"/}; argyll_mount= ;;
    *) argyll_in=/src/argyllcms; argyll_mount="-v $ARGYLL_SRC:/src/argyllcms:ro" ;;
esac

# Read-only sources; the build works in its own copies. SELinux labelling is
# off for the container rather than relabelling with ":Z", which would
# change the labels of the source trees.
"$ENGINE" run --rm --security-opt label=disable \
    -v "$root:/src/displaycraft:ro" \
    $argyll_mount \
    -v "$CACHE:/cache" \
    -v "$OUT:/out" \
    -e SRC=/src/displaycraft -e ARGYLL_SRC="$argyll_in" -e CACHE=/cache -e OUT=/out \
    docker.io/library/ubuntu:24.04 \
    /bin/bash /src/displaycraft/packaging/appimage/make-appimage.sh
ls -l "$OUT"/DisplayCraft-*.AppImage
