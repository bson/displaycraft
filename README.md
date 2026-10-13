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
which shows test patches and loads calibrations through KWin.

DisplayCraft is not affiliated with ArtCraft.

## Requirements

- KDE Plasma on Wayland. ArgyllCMS checks this when DisplayCraft starts, and
  DisplayCraft won't run anywhere it reports the platform as unsupported.
- ArgyllCMS. Its directory can be set in Preferences; otherwise it's looked
  for in `DISPLAYCRAFT_ARGYLL_BIN`, `/opt/argyll-kde-wayland/bin`, then on
  `PATH`.
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
Plasma 6.4 or later. The Wayland ArgyllCMS is the `argyllcms` submodule
([bson/ArgyllCMS](https://github.com/bson/ArgyllCMS)):

    git submodule update --init
    packaging/appimage/build.sh

The AppImage is written to `dist/`. On GitHub, the Build workflow makes a
test build when run from the Actions tab, and publishes a release with the
AppImage when a version tag (`v0.1.0`, matching `Cargo.toml`) is pushed.

Measuring instruments need a udev rule giving the user access. Where the
system has none, DisplayCraft offers to install ArgyllCMS's rule, asking
for an administrator password.

## License

MIT or Apache-2.0, at your option. See NOTICE for the third-party material
included.
