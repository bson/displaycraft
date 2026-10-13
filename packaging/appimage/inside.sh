#!/bin/bash
# Runs in the container started by build.sh. Builds ArgyllCMS and
# DisplayCraft, then the AppImage.
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# No devel packages for TIFF, JPEG, PNG, zlib or SSL: ArgyllCMS builds its
# own copies in, so its programs need only libwayland-client and libdbus,
# which every Plasma system has
apt-get install -y -qq --no-install-recommends \
    build-essential jam pkg-config libwayland-dev libdbus-1-dev \
    curl ca-certificates git file >/dev/null

# Copy a source tree as git sees it: tracked and new files, uncommitted
# changes included, but nothing ignored. Host build output (DisplayCraft's
# target/, ArgyllCMS's in-tree objects) would otherwise come along, and jam
# would link the host's objects, built for its newer glibc.
git config --global --add safe.directory '*'
# --no-recursion: a submodule is listed as one entry, and is copied on its own
copy_tree() {
    mkdir -p "$2"
    git -C "$1" ls-files -co --exclude-standard -z | tar -C "$1" --null --no-recursion -T - -cf - | tar -C "$2" -xf -
}

export RUSTUP_HOME=/cache/rustup CARGO_HOME=/cache/cargo
if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
fi
export PATH="$CARGO_HOME/bin:$PATH"
rustup -q update stable

# ArgyllCMS
rm -rf /build && mkdir -p /build
copy_tree "$ARGYLL_IN" /build/argyllcms
argyll_commit=$(git -C "$ARGYLL_IN" rev-parse HEAD)
argyll_dirty=$(git -C "$ARGYLL_IN" status --porcelain --untracked-files=no | head -c1)
cd /build/argyllcms
export OSTYPE MACHTYPE HOSTTYPE
jam -q -fJambase -j"$(nproc)" \
    -sBUILTIN_TIFF=true -sBUILTIN_JPEG=true -sBUILTIN_PNG=true -sBUILTIN_Z=true -sBUILTIN_SSL=true \
    -sWLPROTO_SYS=/proto/sys -sWLPROTO_KDE=/proto/kde \
    -sDESTDIR=/build/argyll install >/build/argyll-build.log 2>&1 \
    || { tail -40 /build/argyll-build.log; exit 1; }

# DisplayCraft, from a copy so the read-only source isn't written to
copy_tree /src/displaycraft /build/displaycraft
cd /build/displaycraft
CARGO_TARGET_DIR=/cache/target cargo build --release --locked -q
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

# The AppDir
app=/build/AppDir
mkdir -p "$app/usr/bin" "$app/usr/lib/argyll/bin" "$app/usr/share/applications" "$app/usr/share/doc/displaycraft"
cp /cache/target/release/displaycraft "$app/usr/bin/"
# The ArgyllCMS programs DisplayCraft runs, and a few for instrument setup
# and corrections from the command line
for p in dispcal dispread dispwin targen colprof fakeread colverify spotread ccxxmake oeminst iccdump; do
    cp "/build/argyll/bin/$p" "$app/usr/lib/argyll/bin/"
done
strip "$app"/usr/lib/argyll/bin/* "$app/usr/bin/displaycraft"
# Vendor colorimeter corrections; DisplayCraft looks beside the bin folder
cp -a /build/argyll/ref "$app/usr/lib/argyll/ref"
cp -a assets/icons "$app/usr/share/"
cp assets/displaycraft.desktop "$app/usr/share/applications/"
cp assets/displaycraft.desktop "$app/"
cp assets/icons/hicolor/256x256/apps/displaycraft.png "$app/"
ln -sf displaycraft.png "$app/.DirIcon"
cp LICENSE-MIT LICENSE-APACHE NOTICE "$app/usr/share/doc/displaycraft/"
cp /build/argyllcms/License.txt "$app/usr/share/doc/displaycraft/ArgyllCMS-LICENSE.txt"
# Which source this was built from, as the AGPL asks
{
    cat packaging/appimage/ARGYLLCMS.txt
    echo "    Commit: $argyll_commit${argyll_dirty:+ (with uncommitted changes, so not exactly this source)}"
} >"$app/usr/share/doc/displaycraft/ARGYLLCMS.txt"
install -m 0755 packaging/appimage/AppRun "$app/AppRun"

# Built for glibc 2.39 at most, or it won't run where it should
newest=$(objdump -T "$app/usr/bin/displaycraft" "$app"/usr/lib/argyll/bin/* | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1)
echo "Newest glibc symbol needed: $newest"

if [ ! -x /cache/appimagetool ]; then
    curl -sSfL -o /cache/appimagetool \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x /cache/appimagetool
fi
# No FUSE in the container, so appimagetool runs extracted
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 /cache/appimagetool --no-appstream "$app" "/out/DisplayCraft-$version-x86_64.AppImage"
