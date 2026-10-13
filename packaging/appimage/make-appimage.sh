#!/bin/bash
# Build DisplayCraft and the Wayland ArgyllCMS, then the AppImage. Runs in
# the Ubuntu 24.04 container build.sh starts, or directly on an Ubuntu 24.04
# machine such as a GitHub Actions runner (see .github/workflows/build.yml).
#
# Settings, from the environment:
#   SRC          this repository (default: the one this script is in)
#   ARGYLL_SRC   the Wayland ArgyllCMS (default: $SRC/argyllcms, the submodule)
#   CACHE        Rust, crates, build output and appimagetool, kept between
#                builds (default: $SRC/tmp/appimage-cache)
#   OUT          where the AppImage goes (default: $SRC/dist)
#   WORK         scratch space, emptied first (default: a folder under /tmp)
set -euo pipefail

SRC=${SRC:-$(cd "$(dirname "$0")/../.." && pwd)}
ARGYLL_SRC=${ARGYLL_SRC:-$SRC/argyllcms}
CACHE=${CACHE:-$SRC/tmp/appimage-cache}
OUT=${OUT:-$SRC/dist}
WORK=${WORK:-/tmp/displaycraft-appimage}
mkdir -p "$CACHE" "$OUT"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -1)
# A release tag must name this version, or the release and the AppImage
# would disagree
if [ "${GITHUB_REF_TYPE:-}" = tag ] && [ "${GITHUB_REF_NAME:-}" != "v$version" ]; then
    echo "Tag ${GITHUB_REF_NAME} doesn't match version $version in Cargo.toml" >&2
    exit 1
fi

sudo=$([ "$(id -u)" = 0 ] || echo sudo)
export DEBIAN_FRONTEND=noninteractive
$sudo apt-get update -qq
# No devel packages for TIFF, JPEG, PNG, zlib or SSL: ArgyllCMS builds its
# own copies in, so its programs need only libwayland-client and libdbus,
# which every Plasma system has. Its Wayland protocol definitions come with
# it (wlproto/), as Ubuntu 24.04's are too old.
$sudo apt-get install -y -qq --no-install-recommends \
    build-essential jam pkg-config libwayland-dev libdbus-1-dev \
    curl ca-certificates git file >/dev/null

# Rust: an installed rustup (as on GitHub's runners), or one kept in the cache
if ! command -v rustup >/dev/null; then
    export RUSTUP_HOME=$CACHE/rustup CARGO_HOME=$CACHE/cargo
    [ -x "$CARGO_HOME/bin/rustup" ] || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
    export PATH="$CARGO_HOME/bin:$PATH"
fi
rustup -q update stable
rustup -q default stable

# Copy a source tree as git sees it: tracked and new files, uncommitted
# changes included, but nothing ignored. Host build output (DisplayCraft's
# target/, ArgyllCMS's in-tree objects) would otherwise come along, and jam
# would link the host's objects, built for its newer glibc. --no-recursion:
# a submodule is listed as one entry, and is copied on its own.
git config --global --add safe.directory '*'
copy_tree() {
    mkdir -p "$2"
    git -C "$1" ls-files -co --exclude-standard -z | tar -C "$1" --null --no-recursion -T - -cf - | tar -C "$2" -xf -
}

rm -rf "$WORK" && mkdir -p "$WORK"

# ArgyllCMS
copy_tree "$ARGYLL_SRC" "$WORK/argyllcms"
argyll_commit=$(git -C "$ARGYLL_SRC" rev-parse HEAD)
argyll_dirty=$(git -C "$ARGYLL_SRC" status --porcelain --untracked-files=no | head -c1)
cd "$WORK/argyllcms"
export OSTYPE MACHTYPE HOSTTYPE
jam -q -fJambase -j"$(nproc)" \
    -sBUILTIN_TIFF=true -sBUILTIN_JPEG=true -sBUILTIN_PNG=true -sBUILTIN_Z=true -sBUILTIN_SSL=true \
    -sDESTDIR="$WORK/argyll" install >"$WORK/argyll-build.log" 2>&1 \
    || { tail -40 "$WORK/argyll-build.log"; exit 1; }

# DisplayCraft, from a copy so the source isn't written to
copy_tree "$SRC" "$WORK/displaycraft"
cd "$WORK/displaycraft"
CARGO_TARGET_DIR=$CACHE/target cargo build --release --locked -q

# The AppDir
app=$WORK/AppDir
mkdir -p "$app/usr/bin" "$app/usr/lib/argyll/bin" "$app/usr/share/applications" "$app/usr/share/doc/displaycraft"
cp "$CACHE/target/release/displaycraft" "$app/usr/bin/"
# The ArgyllCMS programs DisplayCraft runs, and a few for instrument setup
# and corrections from the command line
for p in dispcal dispread dispwin targen colprof fakeread colverify spotread ccxxmake oeminst iccdump; do
    cp "$WORK/argyll/bin/$p" "$app/usr/lib/argyll/bin/"
done
strip "$app"/usr/lib/argyll/bin/* "$app/usr/bin/displaycraft"
# Vendor colorimeter corrections; DisplayCraft looks beside the bin folder
cp -a "$WORK/argyll/ref" "$app/usr/lib/argyll/ref"
cp -a assets/icons "$app/usr/share/"
cp assets/displaycraft.desktop "$app/usr/share/applications/"
cp assets/displaycraft.desktop "$app/"
cp assets/icons/hicolor/256x256/apps/displaycraft.png "$app/"
ln -sf displaycraft.png "$app/.DirIcon"
cp LICENSE-MIT LICENSE-APACHE NOTICE "$app/usr/share/doc/displaycraft/"
cp "$WORK/argyllcms/License.txt" "$app/usr/share/doc/displaycraft/ArgyllCMS-LICENSE.txt"
# Which source this was built from, as the AGPL asks
{
    cat packaging/appimage/ARGYLLCMS.txt
    echo "    Commit: $argyll_commit${argyll_dirty:+ (with uncommitted changes, so not exactly this source)}"
} >"$app/usr/share/doc/displaycraft/ARGYLLCMS.txt"
install -m 0755 packaging/appimage/AppRun "$app/AppRun"

# Built for glibc 2.39 at most, or it won't run where it should
newest=$(objdump -T "$app/usr/bin/displaycraft" "$app"/usr/lib/argyll/bin/* | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1)
echo "Newest glibc symbol needed: $newest"

if [ ! -x "$CACHE/appimagetool" ]; then
    curl -sSfL -o "$CACHE/appimagetool" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$CACHE/appimagetool"
fi
# Extracted rather than mounted: containers and runners may lack FUSE
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$CACHE/appimagetool" --no-appstream "$app" "$OUT/DisplayCraft-$version-x86_64.AppImage"
