#!/bin/bash
# Build DisplayCraft and the modified ArgyllCMS, then the AppImage. Runs in
# the Ubuntu 24.04 container build.sh starts, or directly on an Ubuntu 24.04
# machine such as a GitHub Actions runner (see .github/workflows/build.yml).
#
#   packaging/appimage/make-appimage.sh [options]
#
#   --src DIR          this repository (default: the one this script is in)
#   --argyll-src DIR   the modified ArgyllCMS (default: <src>/argyllcms, the
#                      submodule)
#   --cache DIR        Rust, crates, build output and appimagetool, kept
#                      between builds (default: <src>/tmp/appimage-cache)
#   --out DIR          where the AppImage goes (default: <src>/dist)
#   --work DIR         scratch space, emptied first
#                      (default: <src>/tmp/appimage-work)
#   --rust-home DIR    where rustup put .cargo and .rustup (default: the
#                      user's home folder; without a rustup there, one is
#                      installed in the cache)
#   --release-tag TAG  a release build for TAG, which must be v<version in
#                      Cargo.toml>
#
# Nothing is taken from the environment: the script starts itself again in
# one made from these options, so a shell's settings can't change the build
# (jam, for one, makes every environment variable a build setting).
set -euo pipefail

inner=false
if [ "${1:-}" = --in-clean-env ]; then
    inner=true
    shift
fi
src=$(cd "$(dirname "$0")/../.." && pwd)
argyll_src= cache= out= work= release_tag=
# From the user database rather than $HOME
home=$(eval echo "~$(id -un)")
rust_home=$home
while [ $# -gt 0 ]; do
    case $1 in
        --src) src=$2 ;;
        --argyll-src) argyll_src=$2 ;;
        --cache) cache=$2 ;;
        --out) out=$2 ;;
        --work) work=$2 ;;
        --rust-home) rust_home=$2 ;;
        --release-tag) release_tag=$2 ;;
        *) echo "Unknown option $1 (see the top of $0)" >&2; exit 2 ;;
    esac
    shift 2
done
src=$(cd "$src" && pwd)
argyll_src=$(cd "${argyll_src:-$src/argyllcms}" && pwd)
cache=$(mkdir -p "${cache:-$src/tmp/appimage-cache}" && cd "${cache:-$src/tmp/appimage-cache}" && pwd)
out=$(mkdir -p "${out:-$src/dist}" && cd "${out:-$src/dist}" && pwd)
work=$(mkdir -p "${work:-$src/tmp/appimage-work}" && cd "${work:-$src/tmp/appimage-work}" && pwd)

if ! $inner; then
    # An installed rustup (as on GitHub's runners), or one kept in the cache
    if [ -x "$rust_home/.cargo/bin/rustup" ]; then
        cargo_home=$rust_home/.cargo rustup_home=$rust_home/.rustup
    else
        cargo_home=$cache/cargo rustup_home=$cache/rustup
    fi
    mkdir -p "$cache/tmp"
    # git's global settings (ignore rules among them) would change what's
    # copied, and the safe.directory set below mustn't go into the user's
    exec /usr/bin/env -i \
        HOME="$home" \
        PATH="$cargo_home/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        CARGO_HOME="$cargo_home" RUSTUP_HOME="$rustup_home" \
        TMPDIR="$cache/tmp" GIT_CONFIG_GLOBAL="$cache/gitconfig" \
        /bin/bash "$0" --in-clean-env --src "$src" --argyll-src "$argyll_src" --cache "$cache" \
        --out "$out" --work "$work" --rust-home "$rust_home" --release-tag "$release_tag"
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$src/Cargo.toml" | head -1)
# A release must be of this version, or the release and the AppImage would
# disagree
if [ -n "$release_tag" ] && [ "$release_tag" != "v$version" ]; then
    echo "Tag $release_tag doesn't match version $version in Cargo.toml" >&2
    exit 1
fi

# sudo drops the environment, so apt's setting goes on its command line
sudo=$([ "$(id -u)" = 0 ] || echo sudo)
apt="$sudo env DEBIAN_FRONTEND=noninteractive apt-get"
$apt update -qq
# No devel packages for TIFF, JPEG, PNG, zlib or SSL: ArgyllCMS builds its
# own copies in. Its programs need libwayland-client, libdbus and libX11
# from the system, which every desktop with Wayland or X has, and the X
# extension libraries bundled below. Its Wayland protocol definitions come
# with it (wlproto/), as Ubuntu 24.04's are too old.
$apt install -y -qq --no-install-recommends \
    build-essential jam pkg-config libwayland-dev libdbus-1-dev \
    libx11-dev libxext-dev libxxf86vm-dev libxinerama-dev libxrandr-dev libxss-dev \
    patchelf curl ca-certificates git file >/dev/null

# Rust: CARGO_HOME and RUSTUP_HOME were chosen above; install it there if
# it isn't yet
[ -x "$CARGO_HOME/bin/rustup" ] || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
rustup -q update stable
rustup -q default stable

# Copy a source tree as git sees it: tracked and new files, uncommitted
# changes included, but nothing ignored. Host build output (DisplayCraft's
# target/, ArgyllCMS's in-tree objects) would otherwise come along, and jam
# would link the host's objects, built for its newer glibc. --no-recursion:
# a submodule is listed as one entry, and is copied on its own. The
# sources may belong to another user (a container's mount), which git
# refuses without safe.directory; this goes in the cache's git config.
git config --global safe.directory '*'
copy_tree() {
    mkdir -p "$2"
    # Tracked files deleted in the tree are still listed, so they're skipped
    git -C "$1" ls-files -co --exclude-standard -z \
        | while IFS= read -r -d '' f; do if [ -e "$1/$f" ]; then printf '%s\0' "$f"; fi; done \
        | tar -C "$1" --null --no-recursion -T - -cf - | tar -C "$2" -xf -
}

rm -rf "$work" && mkdir -p "$work"

# ArgyllCMS
copy_tree "$argyll_src" "$work/argyllcms"
argyll_commit=$(git -C "$argyll_src" rev-parse HEAD)
argyll_dirty=$(git -C "$argyll_src" status --porcelain --untracked-files=no | head -c1)
cd "$work/argyllcms"
export OSTYPE MACHTYPE HOSTTYPE
jam -q -fJambase -j"$(nproc)" \
    -sBUILTIN_TIFF=true -sBUILTIN_JPEG=true -sBUILTIN_PNG=true -sBUILTIN_Z=true -sBUILTIN_SSL=true \
    -sDESTDIR="$work/argyll" install >"$work/argyll-build.log" 2>&1 \
    || { tail -40 "$work/argyll-build.log"; exit 1; }

# DisplayCraft, from a copy so the source isn't written to
copy_tree "$src" "$work/displaycraft"
cd "$work/displaycraft"
CARGO_TARGET_DIR=$cache/target cargo build --release --locked -q

# The AppDir
app=$work/AppDir
mkdir -p "$app/usr/bin" "$app/usr/lib/argyll/bin" "$app/usr/share/applications" "$app/usr/share/doc/displaycraft"
cp "$cache/target/release/displaycraft" "$app/usr/bin/"
# The ArgyllCMS programs DisplayCraft runs, and a few for instrument setup
# and corrections from the command line
for p in dispcal dispread dispwin targen colprof fakeread colverify spotread ccxxmake oeminst iccdump; do
    cp "$work/argyll/bin/$p" "$app/usr/lib/argyll/bin/"
done
strip "$app"/usr/lib/argyll/bin/* "$app/usr/bin/displaycraft"
# X extension libraries, which a Wayland-only system may lack. Found through
# an rpath rather than LD_LIBRARY_PATH in AppRun, which DisplayCraft would
# pass on to every program it starts. libX11 and libxcb aren't bundled
# (AppImage practice): the system's must match its X server.
mkdir -p "$app/usr/lib/argyll/lib"
for l in libXext.so.6 libXxf86vm.so.1 libXinerama.so.1 libXrandr.so.2 libXrender.so.1 libXss.so.1; do
    cp -L "$(ldconfig -p | awk -v l="$l" '$1 == l && /x86-64/ { print $NF; exit }')" "$app/usr/lib/argyll/lib/"
done
for f in "$app"/usr/lib/argyll/bin/*; do
    patchelf --set-rpath '$ORIGIN/../lib' "$f"
done
# A runpath only serves the file's own dependencies, so libXrandr needs one
# to find libXrender
for f in "$app"/usr/lib/argyll/lib/*; do
    patchelf --set-rpath '$ORIGIN' "$f"
done
# Vendor colorimeter corrections; DisplayCraft looks beside the bin folder
cp -a "$work/argyll/ref" "$app/usr/lib/argyll/ref"
cp -a assets/icons "$app/usr/share/"
cp assets/displaycraft.desktop "$app/usr/share/applications/"
cp assets/displaycraft.desktop "$app/"
cp assets/icons/hicolor/256x256/apps/displaycraft.png "$app/"
ln -sf displaycraft.png "$app/.DirIcon"
cp LICENSE-MIT LICENSE-APACHE NOTICE "$app/usr/share/doc/displaycraft/"
cp "$work/argyllcms/License.txt" "$app/usr/share/doc/displaycraft/ArgyllCMS-LICENSE.txt"
# Which source this was built from, as the AGPL asks
{
    cat packaging/ARGYLLCMS.txt
    echo "    Commit: $argyll_commit${argyll_dirty:+ (with uncommitted changes, so not exactly this source)}"
} >"$app/usr/share/doc/displaycraft/ARGYLLCMS.txt"
install -m 0755 packaging/appimage/AppRun "$app/AppRun"

# Built for glibc 2.39 at most, or it won't run where it should
newest=$(objdump -T "$app/usr/bin/displaycraft" "$app"/usr/lib/argyll/bin/* | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1)
echo "Newest glibc symbol needed: $newest"

if [ ! -x "$cache/appimagetool" ]; then
    curl -sSfL -o "$cache/appimagetool" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$cache/appimagetool"
fi
# Extracted rather than mounted: containers and runners may lack FUSE
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$cache/appimagetool" --no-appstream "$app" "$out/DisplayCraft-$version-x86_64.AppImage"
