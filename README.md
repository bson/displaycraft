# DisplayCraft

Display calibration and profiling for Linux desktops, written in Rust with the
look of the ArtCraft applications (egui with the ArtCraft `craft-ui` library).
It walks through picking a display and instrument (with a colorimeter
correction if wanted), setting calibration targets, then calibrating, making
an ICC profile (shaper+matrix or LUT, from a set of measured patches) and
installing it, and verifying how accurate a profile is.
Before calibrating, a monitor's own white point, brightness and black level
controls can be adjusted with live readings. Verification gives a printable
HTML report with charts, and results can be exported as CSV. Profiles can be
viewed as graphs: calibration curves, tonal response and gamut. A display report measures
brightness, contrast, gamma, white point and gamut coverage, uncalibrated
or as calibrated, and a profiles page shows which profile KWin is using
and switches, removes or deletes them.

The measuring is done by ArgyllCMS, which DisplayCraft runs as separate
programs. On KDE Plasma under Wayland it needs the Wayland build of ArgyllCMS,
which shows test patches and loads calibrations through KWin. That build
also keeps ArgyllCMS's X11 support, used on X11 sessions and, through
XWayland, on other Wayland desktops.

Its About page links to the source of DisplayCraft and of the modified
ArgyllCMS it comes with.

DisplayCraft is not affiliated with ArtCraft.

## Requirements

- KDE Plasma on Wayland, which is what DisplayCraft is developed and tested
  on. X11 sessions (Plasma, GNOME) and other Wayland desktops should work
  through ArgyllCMS's X11 support, but haven't been tried yet; which
  profile is in use comes from KDE. ArgyllCMS checks the
  platform when DisplayCraft starts, and DisplayCraft won't run anywhere it
  reports the platform as unsupported.
- ArgyllCMS. The AppImage comes with its own, which works with KDE Plasma on
  Wayland or with X11. It finds out which of the two this session calls for
  when DisplayCraft starts; Preferences shows what it found, and can set it
  to one of them, or to another ArgyllCMS installed elsewhere. Run from
  source, the bundled ArgyllCMS is the one in `DISPLAYCRAFT_ARGYLL_BIN` or
  `/opt/argyll-kde-wayland/bin`.
- A supported measuring instrument, with permission to access it (the
  distribution's ArgyllCMS package installs the udev rule for this).
- Rust 1.95 or later to build.

## Building and running

    cargo run --release

To install it for the current user, with its icon and an entry in the
application launcher:

    ./install.sh               # or PREFIX=/usr/local ./install.sh
    ./install.sh --uninstall

Calibrations and profiles are saved in `~/.local/share/displaycraft/`,
verification reports in its `verify/` folder, display reports in
`reports/`, and
preferences in `~/.config/displaycraft/settings.toml`.

## AppImage

`packaging/appimage/build.sh` builds a single AppImage containing
DisplayCraft and the Wayland build of ArgyllCMS, in an Ubuntu 24.04
container (podman or docker), so it runs on any distribution with KDE
Plasma 6.4 or later, or with an X server or XWayland. The Wayland ArgyllCMS is the `argyllcms` submodule
([bson/ArgyllCMS](https://github.com/bson/ArgyllCMS)):

    git submodule update --init
    packaging/appimage/build.sh

The AppImage is written to `dist/`.

Measuring instruments need a udev rule giving the user access. Where the
system has none, DisplayCraft offers to install ArgyllCMS's rule, asking
for an administrator password.

## macOS

`packaging/macos/make-app.sh` builds `DisplayCraft.app`, with the same
ArgyllCMS inside, in a disk image in `dist/`, on a Mac with Xcode's command line
tools and Rust. It builds for the Mac's own processor; GitHub builds for
Apple Silicon. The app isn't signed with an Apple Developer ID, so macOS
asks before the first launch: allow it in System Settings > Privacy &
Security, or run `xattr -dr com.apple.quarantine DisplayCraft.app`. On
macOS DisplayCraft builds and starts, but measuring hasn't been made to
work there yet. Run from source (`cargo run --release`), it uses the
ArgyllCMS inside an installed `DisplayCraft.app`, in `/Applications` or
`~/Applications`.

## Releases

On GitHub, the Build workflow makes test builds of the AppImage and the
macOS app when run from the Actions tab, and publishes a release with both
when a version tag (`v0.1.0`, matching `Cargo.toml`) is pushed. Both are
downloaded as they are, not zipped. A downloaded AppImage needs
`chmod +x` before it runs.

## License

MIT or Apache-2.0, at your option. See NOTICE for the third-party material
included.
