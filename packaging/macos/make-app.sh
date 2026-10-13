#!/bin/bash
# Build DisplayCraft and the modified ArgyllCMS on macOS, then
# DisplayCraft.app, in a disk image. Runs on a GitHub Actions macOS runner (see
# .github/workflows/build.yml), or on a Mac with Xcode's command line tools
# and Rust. Builds for the Mac's own architecture.
#
# Settings, from the environment:
#   SRC          this repository (default: the one this script is in)
#   ARGYLL_SRC   the modified ArgyllCMS (default: $SRC/argyllcms, the submodule)
#   CACHE        jam and build output, kept between builds
#                (default: $SRC/tmp/macos-cache)
#   OUT          where the disk image goes (default: $SRC/dist)
#   WORK         scratch space, emptied first (default: $SRC/tmp/macos-work)
set -euo pipefail

SRC=${SRC:-$(cd "$(dirname "$0")/../.." && pwd)}
ARGYLL_SRC=${ARGYLL_SRC:-$SRC/argyllcms}
CACHE=${CACHE:-$SRC/tmp/macos-cache}
OUT=${OUT:-$SRC/dist}
WORK=${WORK:-$SRC/tmp/macos-work}
mkdir -p "$CACHE" "$OUT"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -1)
# A release tag must name this version, or the release and the app would
# disagree
if [ "${GITHUB_REF_TYPE:-}" = tag ] && [ "${GITHUB_REF_NAME:-}" != "v$version" ]; then
    echo "Tag ${GITHUB_REF_NAME} doesn't match version $version in Cargo.toml" >&2
    exit 1
fi
arch=$(uname -m)

# Both rustc and ArgyllCMS's compiles take this, so everything runs on the
# same macOS versions
export MACOSX_DEPLOYMENT_TARGET=11.0

# jam: neither Homebrew nor Xcode has it, so build ArgyllCMS's own (Jam 2.5
# with its patches), checked against the archive this was written for.
# Only jam0 is needed: the second stage just rebuilds it with itself, and
# ArgyllCMS names its Jambase with -f anyway.
jam_version=2.5.2-1.3.3
jam_sha256=57c3768bd7954ea97600b823018c1d89419f52e0ea385ac10591ad16716dca26
jam=$CACHE/ajam-$jam_version/jam0
if [ ! -x "$jam" ]; then
    curl -sSfL -o "$CACHE/ajam.tgz" "https://www.argyllcms.com/ajam-$jam_version.tgz"
    echo "$jam_sha256  $CACHE/ajam.tgz" | shasum -a 256 -c -
    tar -C "$CACHE" -xzf "$CACHE/ajam.tgz"
    # Old C, which current clang rejects by default without these
    make -C "$CACHE/ajam-$jam_version" CC=cc \
        CFLAGS="-O2 -Wno-implicit-function-declaration -Wno-implicit-int -Wno-int-conversion" jam0
fi

# Copy a source tree as git sees it: tracked and new files, uncommitted
# changes included, but nothing ignored, so earlier build output in the
# tree doesn't come along. A submodule is listed as one entry and copied on
# its own. Tracked files deleted in the tree are still listed, so they're
# skipped.
copy_tree() {
    mkdir -p "$2"
    git -C "$1" ls-files -co --exclude-standard -z \
        | while IFS= read -r -d '' f; do if [ -e "$1/$f" ]; then printf '%s\0' "$f"; fi; done \
        | tar -C "$1" --null -n -T - -cf - | tar -C "$2" -xf -
}

rm -rf "$WORK" && mkdir -p "$WORK"

# ArgyllCMS, with its own copies of TIFF, JPEG, PNG, zlib and SSL, so the
# app doesn't depend on Homebrew's
copy_tree "$ARGYLL_SRC" "$WORK/argyllcms"
argyll_commit=$(git -C "$ARGYLL_SRC" rev-parse HEAD)
argyll_dirty=$(git -C "$ARGYLL_SRC" status --porcelain --untracked-files=no | head -c1)
cd "$WORK/argyllcms"
export OSTYPE MACHTYPE HOSTTYPE
"$jam" -q -fJambase -j"$(sysctl -n hw.ncpu)" \
    -sBUILTIN_TIFF=true -sBUILTIN_JPEG=true -sBUILTIN_PNG=true -sBUILTIN_Z=true -sBUILTIN_SSL=true \
    -sDESTDIR="$WORK/argyll" install >"$WORK/argyll-build.log" 2>&1 \
    || { tail -60 "$WORK/argyll-build.log"; exit 1; }

# DisplayCraft, from a copy so the source isn't written to
copy_tree "$SRC" "$WORK/displaycraft"
cd "$WORK/displaycraft"
CARGO_TARGET_DIR=$CACHE/target cargo build --release --locked -q

# The app. DisplayCraft finds its ArgyllCMS in Contents/Resources/argyll
# (system::MacOs::bundled_dir), and the corrections beside its bin folder.
app=$WORK/DisplayCraft.app
res=$app/Contents/Resources
mkdir -p "$app/Contents/MacOS" "$res/argyll/bin" "$res/doc"
cp "$CACHE/target/release/displaycraft" "$app/Contents/MacOS/"
for p in dispcal dispread dispwin targen colprof fakeread colverify spotread ccxxmake oeminst iccdump; do
    cp "$WORK/argyll/bin/$p" "$res/argyll/bin/"
done
strip "$res"/argyll/bin/* "$app/Contents/MacOS/displaycraft"
cp -R "$WORK/argyll/ref" "$res/argyll/ref"
cp LICENSE-MIT LICENSE-APACHE NOTICE "$res/doc/"
cp "$WORK/argyllcms/License.txt" "$res/doc/ArgyllCMS-LICENSE.txt"
# Which source this was built from, as the AGPL asks
{
    cat packaging/ARGYLLCMS.txt
    echo "    Commit: $argyll_commit${argyll_dirty:+ (with uncommitted changes, so not exactly this source)}"
} >"$res/doc/ARGYLLCMS.txt"

# The icon, from the PNG sizes there are (no 1024 for 512@2x)
iconset=$WORK/DisplayCraft.iconset
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
    cp "assets/icons/hicolor/${s}x${s}/apps/displaycraft.png" "$iconset/icon_${s}x${s}.png"
done
for s in 16 32 128 256; do
    cp "assets/icons/hicolor/$((s * 2))x$((s * 2))/apps/displaycraft.png" "$iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns -o "$res/DisplayCraft.icns" "$iconset"

cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>DisplayCraft</string>
    <key>CFBundleDisplayName</key>
    <string>DisplayCraft</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.bson.displaycraft</string>
    <key>CFBundleExecutable</key>
    <string>displaycraft</string>
    <key>CFBundleIconFile</key>
    <string>DisplayCraft</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>$MACOSX_DEPLOYMENT_TARGET</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# Only the system's libraries and frameworks, or it won't run elsewhere
if otool -L "$app/Contents/MacOS/displaycraft" "$res"/argyll/bin/* \
    | awk '/^\t/ { print $1 }' | grep -v -e '^/usr/lib/' -e '^/System/Library/' | sort -u | grep .; then
    echo "Linked against libraries that aren't part of macOS (above)" >&2
    exit 1
fi

# An ad-hoc signature: Apple Silicon won't run unsigned code, and strip
# removed the linker's. Not a Developer ID, so Gatekeeper still asks.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

# A disk image rather than a zip: one file to download, keeping the app's
# signature and file modes, opened with a double-click. The Applications
# link is the usual drag-to-install.
dmgroot=$WORK/dmg
mkdir -p "$dmgroot"
cp -R "$app" "$dmgroot/"
ln -s /Applications "$dmgroot/Applications"
dmg=$OUT/DisplayCraft-$version-macos-$arch.dmg
hdiutil create -quiet -ov -volname "DisplayCraft $version" -srcfolder "$dmgroot" -fs HFS+ -format UDZO "$dmg"
ls -l "$dmg"
