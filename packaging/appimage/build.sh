#!/bin/sh
# Build DisplayCraft-<version>-x86_64.AppImage, with the Wayland ArgyllCMS
# inside, in an Ubuntu 24.04 container. Ubuntu 24.04's glibc (2.39) is the
# oldest on a distribution with Plasma 6.4 or later, which the Wayland
# ArgyllCMS needs, so the AppImage runs wherever that does.
#
#   packaging/appimage/build.sh
#
# Settings, from the environment:
#   ARGYLL_SRC   the Wayland ArgyllCMS source (default: the argyllcms
#                submodule; run `git submodule update --init` first)
#   OUT          where the AppImage goes (default: dist/)
#   ENGINE       podman (default) or docker
#   WAYLAND_PROTOCOLS, PLASMA_WAYLAND_PROTOCOLS
#                protocol definition directories, from this machine: the
#                color-management and KDE output protocols ArgyllCMS needs
#                are newer than Ubuntu 24.04's packages (defaults:
#                /usr/share/wayland-protocols, /usr/share/plasma-wayland-protocols)
#
# The container downloads Ubuntu packages, rustup and Rust crates, and
# appimagetool from the AppImage project's GitHub releases. They're kept in
# tmp/appimage-cache, so later builds are quicker.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
ARGYLL_SRC=$(cd "${ARGYLL_SRC:-$root/argyllcms}" && pwd)
OUT=${OUT:-$root/dist}
ENGINE=${ENGINE:-podman}
PROTO_SYS=${WAYLAND_PROTOCOLS:-/usr/share/wayland-protocols}
PROTO_KDE=${PLASMA_WAYLAND_PROTOCOLS:-/usr/share/plasma-wayland-protocols}
CACHE=$root/tmp/appimage-cache

for f in "$ARGYLL_SRC/Jamtop" "$PROTO_SYS/staging/color-management/color-management-v1.xml" \
         "$PROTO_KDE/kde-output-device-v2.xml"; do
    [ -e "$f" ] || { echo "Missing $f" >&2; exit 1; }
done
mkdir -p "$OUT" "$CACHE"

# A submodule's git data is in this repository's .git, so it's reached
# through this repository's mount; a separate checkout gets its own
case "$ARGYLL_SRC/" in
    "$root"/*) argyll_in=/src/displaycraft/${ARGYLL_SRC#"$root"/}; argyll_mount= ;;
    *) argyll_in=/src/argyllcms; argyll_mount="-v $ARGYLL_SRC:/src/argyllcms:ro" ;;
esac

# Read-only sources; the container builds in its own copies. SELinux
# labelling is off for the container rather than relabelling with ":Z",
# which would change the labels of the source trees and fails on /usr/share.
"$ENGINE" run --rm --security-opt label=disable \
    -v "$root:/src/displaycraft:ro" \
    $argyll_mount \
    -e ARGYLL_IN="$argyll_in" \
    -v "$PROTO_SYS:/proto/sys:ro" \
    -v "$PROTO_KDE:/proto/kde:ro" \
    -v "$CACHE:/cache" \
    -v "$OUT:/out" \
    docker.io/library/ubuntu:24.04 \
    /bin/bash /src/displaycraft/packaging/appimage/inside.sh
ls -l "$OUT"/DisplayCraft-*.AppImage
