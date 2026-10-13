#!/bin/sh
# Build DisplayCraft-<version>-x86_64.AppImage, with the modified ArgyllCMS
# inside, in an Ubuntu 24.04 container. Ubuntu 24.04's glibc (2.39) is the
# oldest on a distribution with Plasma 6.4 or later, which the Wayland
# ArgyllCMS needs, so the AppImage runs wherever that does. The steps are
# make-appimage.sh's, which GitHub Actions runs directly.
#
#   git submodule update --init
#   packaging/appimage/build.sh [options]
#
#   --argyll-src DIR   the modified ArgyllCMS (default: the argyllcms
#                      submodule)
#   --out DIR          where the AppImage goes (default: dist/)
#   --engine NAME      podman (default) or docker
#
# The container downloads Ubuntu packages, rustup and Rust crates, and
# appimagetool from the AppImage project's GitHub releases. They're kept in
# tmp/appimage-cache, so later builds are quicker.
#
# Nothing is taken from the environment: the container engine runs with one
# made here, and the build inside gets only the options passed to it.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
argyll_src=$root/argyllcms
out=$root/dist
engine=podman
while [ $# -gt 0 ]; do
    case $1 in
        --argyll-src) argyll_src=$2 ;;
        --out) out=$2 ;;
        --engine) engine=$2 ;;
        *) echo "Unknown option $1 (see the top of $0)" >&2; exit 2 ;;
    esac
    shift 2
done
argyll_src=$(cd "$argyll_src" && pwd)
out=$(mkdir -p "$out" && cd "$out" && pwd)
cache=$root/tmp/appimage-cache
mkdir -p "$cache"

[ -e "$argyll_src/Jamtop" ] || { echo "No ArgyllCMS in $argyll_src: run git submodule update --init" >&2; exit 1; }

# A submodule's git data is in this repository's .git, so it's reached
# through this repository's mount; a separate checkout gets its own
case "$argyll_src/" in
    "$root"/*) argyll_in=/src/displaycraft/${argyll_src#"$root"/}; argyll_mount= ;;
    *) argyll_in=/src/argyllcms; argyll_mount="-v $argyll_src:/src/argyllcms:ro" ;;
esac

# Read-only sources; the build works in its own copies, inside the
# container. SELinux labelling is off for the container rather than
# relabelling with ":Z", which would change the labels of the source trees.
# Rootless podman finds its per-user state through XDG_RUNTIME_DIR, which is
# always /run/user/<uid>.
home=$(eval echo "~$(id -un)")
/usr/bin/env -i HOME="$home" PATH=/usr/local/bin:/usr/bin:/bin XDG_RUNTIME_DIR="/run/user/$(id -u)" \
    "$engine" run --rm --security-opt label=disable \
    -v "$root:/src/displaycraft:ro" \
    $argyll_mount \
    -v "$cache:/cache" \
    -v "$out:/out" \
    docker.io/library/ubuntu:24.04 \
    /bin/bash /src/displaycraft/packaging/appimage/make-appimage.sh \
    --src /src/displaycraft --argyll-src "$argyll_in" --cache /cache --out /out --work /tmp/work
ls -l "$out"/DisplayCraft-*.AppImage
