#!/bin/bash
# Build DisplayCraft and the modified ArgyllCMS on macOS, then
# DisplayCraft.app, in a disk image. Runs on a GitHub Actions macOS runner
# (see .github/workflows/build.yml), or on a Mac with Xcode's command line
# tools and Rust (rustup). Builds for the Mac's own architecture.
#
#   packaging/macos/make-app.sh [options]
#
#   --src DIR          this repository (default: the one this script is in)
#   --argyll-src DIR   the modified ArgyllCMS (default: <src>/argyllcms, the
#                      submodule)
#   --cache DIR        jam and build output, kept between builds
#                      (default: <src>/tmp/macos-cache)
#   --out DIR          where the disk image goes (default: <src>/dist)
#   --work DIR         scratch space, emptied first
#                      (default: <src>/tmp/macos-work)
#   --rust-home DIR    where rustup put .cargo and .rustup (default: the
#                      user's home folder)
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
cache=$(mkdir -p "${cache:-$src/tmp/macos-cache}" && cd "${cache:-$src/tmp/macos-cache}" && pwd)
out=$(mkdir -p "${out:-$src/dist}" && cd "${out:-$src/dist}" && pwd)
work=$(mkdir -p "${work:-$src/tmp/macos-work}" && cd "${work:-$src/tmp/macos-work}" && pwd)

if ! $inner; then
    mkdir -p "$cache/tmp"
    # git's global settings (ignore rules among them) would change what's
    # copied, so it gets a config of its own
    exec /usr/bin/env -i \
        HOME="$home" \
        PATH="$rust_home/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        CARGO_HOME="$rust_home/.cargo" RUSTUP_HOME="$rust_home/.rustup" \
        TMPDIR="$cache/tmp" GIT_CONFIG_GLOBAL="$cache/gitconfig" \
        /bin/bash "$0" --in-clean-env --src "$src" --argyll-src "$argyll_src" --cache "$cache" \
        --out "$out" --work "$work" --rust-home "$rust_home" --release-tag "$release_tag"
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$src/Cargo.toml" | head -1)
# A release must be of this version, or the release and the app would
# disagree
if [ -n "$release_tag" ] && [ "$release_tag" != "v$version" ]; then
    echo "Tag $release_tag doesn't match version $version in Cargo.toml" >&2
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
jam=$cache/ajam-$jam_version/jam0
if [ ! -x "$jam" ]; then
    curl -sSfL -o "$cache/ajam.tgz" "https://www.argyllcms.com/ajam-$jam_version.tgz"
    echo "$jam_sha256  $cache/ajam.tgz" | shasum -a 256 -c -
    tar -C "$cache" -xzf "$cache/ajam.tgz"
    # Old C, which current clang rejects by default without these
    make -C "$cache/ajam-$jam_version" CC=cc \
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

rm -rf "$work" && mkdir -p "$work"

# ArgyllCMS, with its own copies of TIFF, JPEG, PNG, zlib and SSL, so the
# app doesn't depend on Homebrew's
copy_tree "$argyll_src" "$work/argyllcms"
argyll_commit=$(git -C "$argyll_src" rev-parse HEAD)
argyll_dirty=$(git -C "$argyll_src" status --porcelain --untracked-files=no | head -c1)
cd "$work/argyllcms"
# Jambase reads these, which are bash's own rather than the user's
export OSTYPE MACHTYPE HOSTTYPE
"$jam" -q -fJambase -j"$(sysctl -n hw.ncpu)" \
    -sBUILTIN_TIFF=true -sBUILTIN_JPEG=true -sBUILTIN_PNG=true -sBUILTIN_Z=true -sBUILTIN_SSL=true \
    -sDESTDIR="$work/argyll" install >"$work/argyll-build.log" 2>&1 \
    || { tail -60 "$work/argyll-build.log"; exit 1; }
# jam can succeed having built nothing
if [ ! -x "$work/argyll/bin/dispcal" ]; then
    tail -60 "$work/argyll-build.log"
    echo "ArgyllCMS's build installed no programs (log above)" >&2
    exit 1
fi

# DisplayCraft, from a copy so the source isn't written to
copy_tree "$src" "$work/displaycraft"
cd "$work/displaycraft"
CARGO_TARGET_DIR=$cache/target cargo build --release --locked -q

# The app. DisplayCraft finds its ArgyllCMS in Contents/Resources/argyll
# (system::MacOs::bundled_dir), and the corrections beside its bin folder.
app=$work/DisplayCraft.app
res=$app/Contents/Resources
mkdir -p "$app/Contents/MacOS" "$res/argyll/bin" "$res/doc"
cp "$cache/target/release/displaycraft" "$app/Contents/MacOS/"
for p in dispcal dispread dispwin targen colprof fakeread colverify spotread ccxxmake oeminst iccdump; do
    cp "$work/argyll/bin/$p" "$res/argyll/bin/"
done
strip "$res"/argyll/bin/* "$app/Contents/MacOS/displaycraft"
cp -R "$work/argyll/ref" "$res/argyll/ref"
cp LICENSE-MIT LICENSE-APACHE NOTICE "$res/doc/"
cp "$work/argyllcms/License.txt" "$res/doc/ArgyllCMS-LICENSE.txt"
# Which source this was built from, as the AGPL asks
{
    cat packaging/ARGYLLCMS.txt
    echo "    Commit: $argyll_commit${argyll_dirty:+ (with uncommitted changes, so not exactly this source)}"
} >"$res/doc/ARGYLLCMS.txt"

# The icon, from the PNG sizes there are (no 1024 for 512@2x)
iconset=$work/DisplayCraft.iconset
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
dmgroot=$work/dmg
mkdir -p "$dmgroot"
cp -R "$app" "$dmgroot/"
ln -s /Applications "$dmgroot/Applications"
dmg=$out/DisplayCraft-$version-macos-$arch.dmg
hdiutil create -quiet -ov -volname "DisplayCraft $version" -srcfolder "$dmgroot" -fs HFS+ -format UDZO "$dmg"
ls -l "$dmg"
