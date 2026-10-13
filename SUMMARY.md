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
- `src/verify.rs`: the verification patch set, `colverify` parsing,
  scoring and the text report.
- `src/corrections.rs`: finding and labelling `.ccss`/`.ccmx` files.
- `src/session.rs`: when the window can't open at all (no Wayland), prints
  the reason and tries `kdialog`, then `zenity`.
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

## Platform check

Whether DisplayCraft can run is ArgyllCMS's call: the identify step
(`App::refresh_inventory`, at startup and on Refresh/Apply) first runs
`dispwin -Q` (`argyll::check_platform`). Unsupported: the steps are disabled
and the page shows ArgyllCMS's reasons, with Check again and Preferences, so
another ArgyllCMS can still be chosen. Unknown (no `platform` line, i.e. a
dispwin without `-Q`, so not the Wayland build): a warning, and the rest
works. Identification takes about 4 s, mostly `dispcal -?` probing USB and
serial ports for instruments, and runs before the window appears.

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

## Corrections

`corrections::find()` searches `$XDG_DATA_HOME/ArgyllCMS`, DisplayCAL's
`$XDG_DATA_HOME/DisplayCAL/dl` (and its per-instrument subdirectories), each
`$XDG_DATA_DIRS/ArgyllCMS` and `/usr/share/color/argyll/ref`, keeping the
first file of each name. Labels come from the CGATS header: `TECHNOLOGY`, and
`DISPLAY`, or `DESCRIPTOR` unless it's "Not specified" (the key match needs a
following space, so `DISPLAY_TYPE_REFRESH` isn't taken for `DISPLAY`). The
choice is saved as `correction` in settings. `App::instrument_args()` passes
it as `-X`, in place of the display type's `-y`, to both dispcal and
dispread.

## Verification

1. 57 patches: a 21 step grayscale, primaries and secondaries at 100% and
   50%, and the 24 ColorChecker colors (BabelColor sRGB averages).
2. `fakeread profile.icc <base>-expected` predicts them (run directly: it
   takes a fraction of a second).
3. `dispread` measures them as a job, with `-k profile.cal` when the
   calibration file sits next to the profile, so the measurement goes
   through the calibration the profile assumes.
4. `colverify -v 2 -N -k expected.ti3 measured.ti3`: `-N` normalises each
   file to its own white, so a backlight change since calibrating doesn't
   count as error; `-k` gives CIEDE2000. The per patch lines
   (`N: L a b <=> L a b  de X`) are parsed and summarised here, rather than
   taking colverify's totals.
5. Verdict: Good when the average is at most 1.0 and the maximum at most 3.0,
   Acceptable at 2.0 and 5.0, otherwise Poor. Grayscale figures are patches
   1-21.
6. Files go to `$XDG_DATA_HOME/displaycraft/verify/<profile>-verify-<stamp>`
   (`-expected.ti1/.ti3`, `-measured.ti1/.ti3`, `.txt` report).

Measuring needs the Wayland ArgyllCMS's pass-through fix: before it, with
the profile under test installed, patches went through that profile's own
tone curves and grays measured far off (a calibration to D55 showed grays at
x 0.306). With it, the same grays measure neutral at D55.

What remains after that fix is real: profiles that dispcal -o made for a
white point far from the panel's native one (D55 here) predict grays
increasingly redder than their white (a* up to +12 near 85%), and record a
white point (x 0.311) that isn't the calibrated one. Measured grays are
neutral, so the calibration is right and the profile's model of the top end
is wrong. A D65 calibration, close to native, doesn't show it.

Checking the approach: a profile whose own prediction bends from its white
point target near full drive (white at x 0.3087 against grays at 0.335)
verified at average 11.6, while a consistent one verified at 1.9 whether the
backlight was at 70% or 95%.

## Verified

- Builds with Rust 1.99; unit tests cover the usage parser, progress and
  noise handling, and output naming.
- Runs natively on KWin 6.7.5 in the Charcoal theme, finds ArgyllCMS 3.5.0 in
  `/opt/argyll-kde-wayland/bin` and lists the eDP-1 panel.
- A saved `argyll_bin` is used; a nonexistent one gives a clear error and
  empty lists.

- `dispcal -?`'s instrument-specific `-y` list (21 types for the
  i1Display Pro): the first entry on the `-y` line itself, `r|c` style
  alternatives (the first is passed), the `i1D3: ` prefix dropped, and the
  generic `Other: l = LCD, c = CRT` line merged without duplicates.
- `app::hardware_tests::calibrate_profile_and_install` (ignored by default;
  run with `cargo test -- --ignored --nocapture`) drives the real app with
  egui_kittest: Calibrate & profile, Start, Continue at each Argyll prompt,
  Install profile. With the i1Display Pro behind a powered hub, Low quality:
  283 s, white point error 0.33 dE, average neutral error 0.39 dE, and KWin
  switched to the installed copy in ICC mode.

- `app::hardware_verify_tests::verify_profile_through_ui` (ignored by
  default; `DISPLAYCRAFT_TEST_PROFILE` picks the profile) drives the Verify
  page. For the 18:29 low quality matrix profile: average 1.89, maximum 7.96
  (saturated ColorChecker patches), grayscale 0.72/1.60, in 58 s; a manual
  run of the same pipeline gave 1.90/7.92.

Not yet tested by hand: the Preferences dialog (opening, Browse, Apply), and
the correction list and Browse on the Instrument page.
