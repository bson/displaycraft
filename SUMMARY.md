# Implementation summary

## Structure

- `src/main.rs`: eframe window setup (wgpu, Wayland, app id `displaycraft`).
- `src/app.rs`: the workflow UI. A step sidebar (Display, Instrument,
  Targets, Calibrate & profile) and a page per step. `logic()` drains the
  running tool's events each frame; `ui()` draws the panels.
- `src/argyll.rs`: finding and running ArgyllCMS.
- `src/settings.rs`: preferences (`argyll_bin`, `theme`) in
  `$XDG_CONFIG_HOME/displaycraft/settings.toml`. Unknown or missing fields
  fall back to defaults, and an unreadable file gives the defaults rather
  than stopping the app. Saves write a `.tmp` file, fsync it and rename it
  into place. (`craft-settings` would fit, but it isn't in the pinned
  craft-libs revision.)
- `src/theme.rs`: LightCraft's four themes (Charcoal, Midnight, Silver,
  Paper), Inter fonts, and egui styling through
  `craft_ui::theme::WidgetPalette`.

## Dependencies

- eframe/egui 0.36 (`wgpu`, `wayland`, `default_fonts` only).
- `craft-ui` from `storytold/craft-libs`, git only and without stability
  guarantees, so pinned to rev `5c874d5`, the one LightCraft uses. It needs
  Rust 1.95+. It supplies components (icon buttons, tabs, toolbar/split
  layout, docking, widget-state styling) but no complete theme, icons or
  fonts: those come from each app, hence the adapted `theme.rs`. LightCraft's
  own `lightcraft-ui-egui` can't be used as a dependency because it pulls in
  nine of LightCraft's engine crates.
- `chrono` for local-time file names.

## Running ArgyllCMS

- Binaries: the Preferences directory if set, otherwise
  `$DISPLAYCRAFT_ARGYLL_BIN`, `/opt/argyll-kde-wayland/bin`, then `PATH`. A
  set directory is used even when it's wrong, so the error shows instead of
  another ArgyllCMS being used silently. Preferences checks a candidate with
  `argyll::probe()` (a `dispcal` that runs and reports a version) and only
  enables Apply when it passes; Apply is also disabled during a run. Browse
  uses rfd's XDG portal folder picker on its own thread, so the UI keeps
  drawing.
- Inventory comes from parsing `dispcal -?`: the version line, the `-d`
  display list, the `-c` instrument list (entries without a parenthesised
  description are bare serial ports Argyll lists whether or not anything is
  attached, and are dropped), and the `-y` display types (either an
  instrument specific list on following lines, or the `Other: l = LCD, c =
  CRT` fallback).
- `Job::start` runs a tool under `stdbuf -o0 -e0` with piped stdin, stdout
  and stderr, and turns its output into events: lines (split on `\r` and
  `\n`), `patch N of M` progress, and prompts. Prompts have no trailing
  newline, so the unterminated remainder is checked for "any other key" or
  a trailing "continue:". Argyll's `next_con_char`/`poll_con_char`
  `tcgetattr` warnings (stdin isn't a terminal) are printed without line
  breaks, so they're cut out of the middle of lines.
- Answering a prompt writes one key to stdin: space to continue, `q` to give
  up. Cancelling sends SIGTERM, so dispwin's signal handler restores the
  display's color settings.

## Calibrate & profile

`dispcal -v -d D -c C [-y T] -m -q Q [-t K] -g G|s [-b B] -o base.icc base`.
`-m` skips the interactive monitor-controls adjustment. `-o` takes an
optional file name, so the profile name is always given explicitly;
otherwise it would consume the output base name that follows. Output goes to
`$XDG_DATA_HOME/displaycraft/<output>-<YYYYMMDD-HHMMSS>.{cal,icc}`. The
result shows dispcal's final error summary lines; "Install profile" runs
`dispwin -d D -I base.icc`, which with the Wayland ArgyllCMS installs it via
ucmm and points KWin at the installed copy.

## Verified

- Builds with Rust 1.99; unit tests cover the usage parser, progress and
  noise handling, and output naming.
- Runs natively on KWin 6.7.5 in the Charcoal theme, finds ArgyllCMS 3.5.0 in
  `/opt/argyll-kde-wayland/bin` and lists the eDP-1 panel.
- A saved `argyll_bin` is used; a nonexistent one gives a clear error and
  empty lists.

Not yet tested: the Preferences dialog itself (opening, Browse, Apply), a
calibration run through the UI, prompts, install, and the instrument-specific
display type list.
