# Implementation summary

## Structure

- `src/main.rs`: eframe window setup (wgpu, Wayland, title `DisplayCraft`,
  app id `displaycraft`).
- `src/app.rs`: the workflow UI. A step sidebar (numbered Display,
  Spectro, Settings, Measure, Verify, then Report and Profiles below a
  separator) and a page per step. Internally the
  steps keep their older names (`Step::Instrument`, `Step::Targets`,
  `Step::Run`). Pages put one and a half text rows (`App::gap()`) after the
  heading, the description and the choices. The top bar shows
  `DisplayCraft <version>` from `CARGO_PKG_VERSION`; the ArgyllCMS version
  and directory are at the bottom of the sidebar. `logic()` drains the
  running tool's events each frame; `ui()` draws the panels.
- `src/argyll.rs`: finding and running ArgyllCMS.
- `src/targets.rs`: calibration targets and their dispcal arguments.
- `src/settings.rs`: preferences (`argyll_bin`, `theme`, `correction`, and
  the last `targets`, saved whenever they change) in
  `$XDG_CONFIG_HOME/displaycraft/settings.toml`. Unknown or missing fields
  fall back to defaults, and an unreadable file gives the defaults rather
  than stopping the app. Saves write a `.tmp` file, fsync it and rename it
  into place. (`craft-settings` would fit, but it isn't in the pinned
  craft-libs revision.)
- `src/verify.rs`: the verification patch set, `colverify` parsing,
  scoring, the text report and CSV export.
- `src/corrections.rs`: finding and labelling `.ccss`/`.ccmx` files.
- `src/report.rs`: the display report's patch set, `.ti3` reading and the
  arithmetic (gamma fit, CCT, u'v' gamut coverage).
- `src/profiles.rs`: reading ICC profiles (description, date, white point),
  listing them, the last verification verdict, and KWin's state for an
  output from `kscreen-doctor -o`.
- `src/session.rs`: when the window can't open at all (no Wayland), prints
  the reason and tries `kdialog`, then `zenity`.
- `src/theme.rs`: LightCraft's four themes (Charcoal, Midnight, Silver,
  Paper), Inter fonts, and egui styling through
  `craft_ui::theme::WidgetPalette`.

## Logo, icon and installing

`assets/displaycraft.png` (1254x1254, opaque dark background) is the source.
`assets/icons/hicolor/<n>x<n>/apps/displaycraft.png` (16 to 512) are made
from it with `magick displaycraft.png -resize <n>x<n> -strip`; regenerate
them when the source changes. The UI embeds the 256 one through
`egui::include_image!` and `egui_extras`'s image loaders (`app::logo()`,
drawn with rounded corners): above the step ladder (once per window, so not
again on the pages), and in the Preferences dialog. `main.rs` also sets it
as the window icon, which matters only when not installed: on Wayland the
compositor takes the icon from the desktop entry matching the app id
(`displaycraft`).

`install.sh` builds a release binary and installs it, the icons and
`assets/displaycraft.desktop` under `PREFIX` (default `~/.local`), with an
absolute `Exec` because the desktop session's `PATH` may lack
`$PREFIX/bin`, then runs `update-desktop-database` and `kbuildsycoca6`.
`--uninstall` removes them. (`gtk-update-icon-cache` is left out: KDE doesn't
use it, and other applications' icons in the user's folder made it fail.)

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
- `egui_extras` (`image`) and `image` (`png` only) for the logo.

## Platform check

Whether DisplayCraft can run is ArgyllCMS's call: the identify step
(`App::refresh_inventory`, at startup and on Refresh/Apply) first runs
`dispwin -Q` (`argyll::check_platform`). Unsupported: the steps are disabled
and the page shows ArgyllCMS's reasons, with Check again and Preferences, so
another ArgyllCMS can still be chosen. Unknown (no `platform` line, i.e. a
dispwin without `-Q`, so not the Wayland build): a warning, and the rest
works. Identification takes about 4 s, mostly `dispcal -?` probing USB and
serial ports for instruments, so it runs on a thread (`App::scan`, taken up
by `poll_scan()` in `logic()`): the window opens at once, Display and
Spectro show a spinner, Refresh is disabled and Start explains the wait. A
new scan replaces a running one, whose result is dropped.

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
- Prompts are answered on the page (`App::job_controls()`), not in a
  dialog. From Start until the first prompt is answered (`placing`), the page
  asks for the instrument to be placed at the center of the screen, with
  Measure (enabled once Argyll has asked) and Cancel. Argyll prints "Place
  instrument on test window." as a complete line, so its placement prompt
  arrives as just the key instructions; with those removed it is shown as
  "Place the measurement device on the patch and press Measure to begin."
  Prompts with text of their own show it. Measure writes a space to stdin.
- Cancelling sends SIGTERM, so dispwin's signal handler restores the
  display's color settings. That handler exits with status 0, so the app
  records the cancel (`cancelled`) rather than trusting the exit status: a
  cancelled calibration deletes whatever `.cal`, `.icc` and `.ti3` it had
  written and the page returns to its start state with no message (a
  cancelled verification likewise), and a calibration only counts
  as done when both its `.cal` and `.icc` exist.

## Settings (targets)

White point choices are D50, D55 and D65 (daylight 5000, 5500, 6500 K),
Native (no `-t`) and Custom, a daylight temperature typed in. Tonal
response is gamma 2.2, gamma 1.8, the sRGB curve or a custom gamma.
`Targets::kelvin()` and `Targets::gamma_arg()` give the `-t` and `-g`
values; `Targets::describe()` gives the "Goals" line on the Measurement
page. Settings shows the chosen display and its backlight correction
(correction file or display type) under the heading; Measurement shows the
instrument (the name from Argyll's description, without the USB path) and
correction. Its log is collapsed by default. Settings rows are spaced out
and wrap (`choices()`; `with_field()` keeps a Custom choice next to its
value).

The Spectro page lists instruments by name, with Argyll's full description
(the USB device) on hover. One dropdown chooses the correction: "None (use
Instrument Default)", the instrument's built-in display types (`-y`), or
correction files (`-X`), with Browse for others. Long entries are truncated
to the field's width and shown whole on hover. The display type choice isn't
saved; a correction file is.

## Measure

`dispcal -v -d D -c C [-y T] -m -q Q [-t K] -g G|s [-b B] -o base.icc base`.
`-m` skips the interactive monitor-controls adjustment. `-o` takes an
optional file name, so the profile name is always given explicitly;
otherwise it would consume the output base name that follows. Output goes to
`$XDG_DATA_HOME/displaycraft/<output>-<YYYYMMDD-HHMMSS>.{cal,icc}`. The
result shows dispcal's final error summary lines; "Install profile" runs
`dispwin -d D -I base.icc`, which with the Wayland ArgyllCMS installs it via
ucmm and points KWin at the installed copy. A successful install makes the
new profile Verify's choice.

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

The page's patch list fills the height the log section leaves. The log's
height is measured each frame and used for the next, with a repaint when it
changes; the open log has a set height (`min_scrolled_height`), otherwise it
would only take the room left and never grow back. "Export CSV…" saves the
per-patch results (`verify::csv()`) through rfd's save dialog on its own
thread, named after the report by default.

Poor results on D55 profiles made earlier came from the Wayland ArgyllCMS
measuring through KWin's ICC mode, which clips single-channel ramps near
full drive; dispcal fitted its profile to those readings, so the profile's
white (x 0.311) disagreed with its grays. ArgyllCMS now measures in KWin's
sRGB mode and applies calibration curves to the patches itself. A D55
profile made with that verified Good: average 0.71, maximum 1.96. Profiles
made before then should be remade.

## Report

`dispread` measures 16 patches (`report::PATCHES`): black three times
(start, and twice at the end; the median is used, as it sets the contrast
ratio), white, grays 10-90% and the three primaries. "Uncalibrated" passes
`-k` with a linear `.cal` (`reports/linear.cal`); "As calibrated now" uses
whatever calibration is in effect. dispread's XYZ is scaled to white
Y = 100, so values are made absolute with `LUMINANCE_XYZ_CDM2`. Gamma is a
least-squares power law through the origin over the grays, black removed.
The primaries' chromaticities have black's XYZ subtracted first. Coverage
of sRGB, Display P3 and Adobe RGB is by area in CIE 1976 u'v'
(Sutherland-Hodgman clipping of the display's triangle against each).
Files go to `$XDG_DATA_HOME/displaycraft/reports/<output>-report-<stamp>`
(`.ti1`, `.ti3`, `.txt`).

## Profiles

Rows are the union of the installed profiles
(`$XDG_CONFIG_HOME/color/icc/devices/display`, where ucmm puts them) and
DisplayCraft's, merged by file name since installing keeps the name.
Description, date and white point come from the profile itself: header date
(UTC, shown local), `desc`/`mluc`, and `wtpt` for v2 profiles only (v4
ones record D50). dispcal describes its profiles by their output path, so
those show the file name. The verdict is the newest `verify/` report's.
What's in use comes from `kscreen-doctor -o` (its JSON leaves out the
profile source): a profile counts only in ICC mode, since after `dispwin -U`
KWin keeps the path but switches to sRGB.

- Use: `dispwin -d D -I path`. ucmm reads the whole file before writing,
  so installing a profile from its own folder is safe, and it deletes the
  replaced installed copy.
- Stop using a profile: `dispwin -d D -U`, which deletes the installed copy
  and returns KWin to sRGB; DisplayCraft's own copy stays.
- Delete (DisplayCraft's profiles not in use, after confirmation): the
  `.icc` and its `.cal`.

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
  egui_kittest: Measure, Start, then Measure or Continue at each Argyll prompt,
  Install profile. With the i1Display Pro behind a powered hub, Low quality:
  283 s, white point error 0.33 dE, average neutral error 0.39 dE, and KWin
  switched to the installed copy in ICC mode.

- `app::hardware_verify_tests::verify_profile_through_ui` (ignored by
  default; `DISPLAYCRAFT_TEST_PROFILE` picks the profile) drives the Verify
  page. For the 18:29 low quality matrix profile: average 1.89, maximum 7.96
  (saturated ColorChecker patches), grayscale 0.72/1.60, in 58 s; a manual
  run of the same pipeline gave 1.90/7.92.

- `app::page_pictures::render_pages` (ignored by default; needs ArgyllCMS
  and a GPU, through egui_kittest's `wgpu` and `snapshot` dev features)
  renders Display, Spectro, Settings, Measurement and Verify, with made-up
  results, to
  `tmp/page-*.png`: Verify at 640 and 900 points tall and with the log
  open. The patch list grew with the window and shrank for the open log.

- Launched from the installed desktop entry: the window appeared at once
  with the spinner, the display list arrived a few seconds later, and KWin
  showed the icon in the title bar.
- Profile switching, run as the page's `dispwin` commands: Use made KWin
  load the chosen profile; Stop returned it to sRGB and removed the
  installed copy; reinstalling the original from DisplayCraft's copy left
  `color.jcnf` byte-identical and the installed folder as before.
- `app::hardware_report_tests::report_through_ui` (ignored by default)
  measures both reports through the page. Uncalibrated: white 233.9 cd/m²,
  black 0.128 (all three readings), 1830:1, gamma 2.21, x 0.3124, y 0.3425;
  dispcal -R on the same panel gave 242.7, 0.130, 1865:1, 2.23, x 0.3109,
  y 0.3416. Primaries x 0.665/0.203/0.143: 100% of sRGB and Adobe RGB, 94%
  of Display P3.

Not yet tested by hand: the Preferences dialog (opening, Browse, Apply), and
the correction list and Browse on the Spectro page, Export CSV, and the
Measure and Cancel buttons while measuring, the Profiles page's buttons
(clicked), and deleting a profile.
