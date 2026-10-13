# displaycraft

Display calibration and profiling for Linux desktops, written in Rust with the
look of the ArtCraft applications (egui with the ArtCraft `craft-ui` library).
It walks through picking a display and instrument (with a colorimeter
correction if wanted), setting calibration targets, then calibrating, making
an ICC profile and installing it, and verifying how accurate a profile is.

The measuring is done by ArgyllCMS, which displaycraft runs as separate
programs. On KDE Plasma under Wayland it needs the Wayland build of ArgyllCMS,
which shows test patches and loads calibrations through KWin.

displaycraft is not affiliated with ArtCraft.

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

Calibrations and profiles are saved in `~/.local/share/displaycraft/`,
verification reports in its `verify/` folder, and
preferences in `~/.config/displaycraft/settings.toml`.

## License

MIT or Apache-2.0, at your option. See NOTICE for the third-party material
included.
