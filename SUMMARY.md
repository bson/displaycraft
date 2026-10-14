# Implementation summary

## Structure

- `src/main.rs`: eframe window setup (wgpu, Wayland or X11, title `DisplayCraft <version>` from `CARGO_PKG_VERSION`,
  app id `displaycraft`).
- `src/app.rs`: the workflow UI. A step sidebar (numbered Display,
  Spectro, Parameters, Measure, Verify, then Report and Profiles below a
  separator) and a page per step. Internally the
  steps keep their older names (`Step::Instrument`, `Step::Targets`,
  `Step::Run`). Pages put one and a half text rows (`App::gap()`) after the
  heading, the description and the choices. There's no top bar: the
  window title carries DisplayCraft's version, and the ArgyllCMS version
  is at the bottom of the sidebar. `logic()` drains the
  running tool's events each frame; `ui()` draws the panels.
- `src/argyll.rs`: finding and running ArgyllCMS.
- `src/system.rs`: what differs between operating systems: the bundled
  ArgyllCMS, where files go, which profile a display uses, and opening
  files and links. A `System` trait with one implementation
  per OS, chosen with `cfg` (`system::current()`): the bundled variants
  Preferences offers (`Variant`: label and display system, the default
  first), where the bundled ArgyllCMS is (`bundled_dir()`), and why it's
  missing. Linux: auto, KDE + Wayland and GNOME, KDE + Xorg, with
  `/opt/argyll-kde-wayland/bin` standing in when run from source. macOS
  (`MacOs`): one "Bundled ArgyllCMS", in `Contents/Resources/argyll/bin`
  of the app the executable is in, or when run from source, of an
  installed `DisplayCraft.app` (`/Applications`, then `~/Applications`).
  Windows (`Single`, not yet packaged): one "Bundled ArgyllCMS", in
  `argyll\bin` beside `displaycraft.exe` (or `$DISPLAYCRAFT_ARGYLL_BIN`). Also `open()`,
  for files and links: xdg-open, `open` on macOS, `cmd /C start ""` on
  Windows. `live_output()` (see Running ArgyllCMS). `data_dir()` and
  `config_dir()`: Linux the XDG base directories plus `displaycraft`
  (`~/.local/share`, `~/.config`); macOS both `~/Library/Application
  Support/DisplayCraft`; Windows `AppData/Roaming/DisplayCraft`.
  `installed_profiles_dir()`: ucmm's `~/.config/color/icc/devices/display`,
  `~/Library/ColorSync/Profiles` (where ArgyllCMS installs for the user),
  Windows' spool `color` folder. `correction_dirs()`: ArgyllCMS's and
  DisplayCAL's data folders on each. `profile_in_use()`: on Linux KWin's
  answer through `kscreen-doctor -o` (which also says whether it's in ICC
  mode at all), elsewhere ArgyllCMS's `dispwin -d n -W`
  (`argyll::installed_profile()`); `color_manager()` ("KDE", "macOS",
  "Windows") and `without_profile()` word the page and messages. Checked to
  compile (`cargo check`) for `aarch64-apple-darwin` and
  `x86_64-pc-windows-gnu`; the kscreen-doctor parsing in `profiles.rs` is
  Linux-only.
- The progress bar (`progress_bar()`) shows Argyll's "patch N of M" as a
  percentage, centred in the bar (painted over it, as egui puts a bar's
  own text at its left end). The patch numbers themselves aren't shown: a
  patch can take several readings, so they don't follow what's on screen.
- The Log (`log_view()`) is one multi-line label, so a selection copies
  with its line breaks (egui joins a selection across labels without
  them), with a Copy log button above it for all of it, since a selection
  can't scroll.
- Windows: release builds are GUI programs (`windows_subsystem` in
  `main.rs`), and every ArgyllCMS program is started with
  `CREATE_NO_WINDOW` (`system::no_console()`, through `argyll::told()`),
  or each would open a console window. Each live program joins a job
  object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (`system::adopt()`,
  windows-sys), so it ends with DisplayCraft however DisplayCraft ends.
  `Job::send_key()` adds a line end there, as ArgyllCMS reads a piped
  stdin a line at a time on Windows. Cancel ends the program outright
  (no signals), so ArgyllCMS doesn't undo display changes as it does on
  SIGTERM elsewhere. Windows' folders are joined a component at a time
  (`roaming()`, `windows_folder()` for `SystemRoot` and `ProgramData`),
  so paths don't mix separators, and the log shows commands without the
  shell prompt's "$" there.
- Messages DisplayCraft shows say "KDE", never "KWin". ArgyllCMS's session
  report is reworded as it's read (`argyll::as_shown()`: "KDE Plasma
  (KWin)" and "KDE Plasma's KWin" become "KDE Plasma", other "KWin" "KDE"),
  and the session line reads e.g. "KDE on Wayland, with XWayland :0" or
  "GNOME on X11 (Xorg :0, window manager GNOME Shell)".
- Files for a display are named after its output (`output_name()`, e.g.
  "eDP-1"), or where the description has none (macOS) after its name with
  anything but letters, digits and "-" made "_" ("Built-in_Display").
- `src/targets.rs`: calibration targets and their dispcal arguments.
- `src/settings.rs`: preferences (`argyll_bin`, `argyll_variant`, `theme`, `correction`, and
  the last `targets`, saved whenever they change) in
  `$XDG_CONFIG_HOME/displaycraft/settings.toml`. Unknown or missing fields
  fall back to defaults, and an unreadable file gives the defaults rather
  than stopping the app. Saves write a `.tmp` file, fsync it and rename it
  into place. (`craft-settings` would fit, but it isn't in the pinned
  craft-libs revision.)
- `src/verify.rs`: the verification patch set, `colverify` parsing,
  scoring, the text report and CSV export.
- `src/corrections.rs`: finding and labelling `.ccss`/`.ccmx` files.
- About (`Step::About`, `page_about()`): DisplayCraft's version and
  licence, "Inspired by DisplayCAL by Florian Höch" (linked), ArgyllCMS (the version in use, its licence, and the modified
  source on GitHub the AGPL calls for), and the theme and font credits.
  Links open with `System::open()` (xdg-open on Linux), as egui's
  hyperlinks need a browser feature eframe isn't built with. The report
  pages' Open buttons use it too. Preferences and About are
  `Step::always_open()`: reachable, and free of ArgyllCMS warnings, where
  ArgyllCMS can't run. During a run their sidebar buttons are disabled,
  like the steps other than the running one's.
- Page headings: Display Selection, Spectrophotometer or Colorimeter,
  Calibration Parameters (the "Parameters" step), Display Measurement,
  Profile Verification. With no instrument connected, Begin is disabled
  with "No instrument." on its own line above it, 1.5 rows apart
  (`NO_INSTRUMENT`); other reasons show beside the button.
- Preferences is a page (`Step::Preferences`, `page_preferences()`), its
  button at the foot of the sidebar, with About under it, above the ArgyllCMS version (no path:
  the AppImage's mount point changes every run, and Preferences shows the
  choice).
  Its state (`Prefs`) lives while the page is open; changes apply at
  once.
- `src/adjust.rs`: reading dispcal's interactive adjustment (menu, mode,
  target and live readings).
- `src/charts.rs`: line, outline, point and bar charts from one description,
  drawn with egui or written as SVG; the CIE 1931 spectral locus.
- `src/report.rs`: the display report's patch set, `.ti3` reading and the
  arithmetic (gamma fit, CCT, u'v' gamut coverage).
- `src/layout.rs`: the arrangement of the displays, from `kscreen-doctor
  -j` or else ArgyllCMS's display list, and `display_name()`.
- `src/profiles.rs`: reading ICC profiles (description, date, white point),
  listing them, the last verification verdict, and (Linux) KWin's state for an
  output from `kscreen-doctor -o`.
- `src/session.rs`: when the window can't open at all (no Wayland), prints
  the reason and tries `kdialog`, then `zenity`.
- `src/theme.rs`: LightCraft's four themes (Charcoal, Midnight, Silver,
  Paper), Inter fonts, and egui styling through
  `craft_ui::theme::WidgetPalette`. Spacing: items 6 × 4, button padding
  8 × 3, and 8 between a radio button's or checkbox's mark and its label
  (`icon_spacing`; egui's 4 crowds the label).

## Logo, icon and installing

`assets/displaycraft.png` (1041x1041, RGBA) is the source: the laptop
cropped to its edges and centred with a 3% margin (it fills 94% of the
width), as Windows shows an icon's transparent margins as they are, so a
smaller laptop looked small there. Its outer background is transparent: the original's dark background (17, 18, 23) was
cleared where it connects to the image's edge, so the laptop's dark screen
stays, and the anti-aliased edge pixels were given partial alpha with the
background unmixed from their colour, so there's no dark fringe on light
backgrounds. `assets/icons/hicolor/<n>x<n>/apps/displaycraft.png` (16 to
512) are made from it with `magick displaycraft.png -resize <n>x<n> -strip
PNG32:<file>` (PNG32 keeps the alpha at small sizes); regenerate
them when the source changes. The UI embeds the 256 one through
`egui::include_image!` and `egui_extras`'s image loaders (`app::logo()`): above the step ladder (once per window, so not
again on the pages). `main.rs` also sets it
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

- eframe/egui 0.36 (`wgpu`, `wayland`, `x11`, `default_fonts` only; winit loads the X11 libraries at run time, so they add no build or link dependency).
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
(`App::refresh_inventory`, at startup, on Search Again (Display and
Spectro) and on every ArgyllCMS
change in Preferences) first runs
`dispwin -Q` (`argyll::check_platform`), parsed into an `argyll::Session`:
desktop, Wayland compositor, X server and window manager, the display
system ArgyllCMS would choose itself (`automatic`) and the one it's using,
and `Platform`. Unsupported: the steps are disabled and the page shows
ArgyllCMS's reasons and the session, with Check again and a button to the
Preferences page (which stays reachable when unsupported), so another
choice can be made. Limited (X11 through XWayland) and Unknown (no
`platform` line, i.e. a dispwin without `-Q`, so not the bundled
ArgyllCMS): a warning above every page but Preferences, and the rest
works. Identification takes about 4 s, mostly `dispcal -?` probing USB and
serial ports for instruments, so it runs on a thread (`App::scan`, taken up
by `poll_scan()` in `logic()`): the window opens at once, Display and
Spectro show a spinner, Search Again is disabled and Begin
explains the wait. A
new scan replaces a running one, whose result is dropped.

## Running ArgyllCMS

- Binaries: the "Other" directory if set (`argyll_bin`), otherwise the
  bundled ArgyllCMS (`System::bundled_dir()`: on Linux
  `$DISPLAYCRAFT_ARGYLL_BIN`, set by AppRun, or
  `/opt/argyll-kde-wayland/bin` when run from source),
  then `PATH`. A set directory is used even when it's wrong, so the error
  shows instead of another ArgyllCMS being used silently.
- The bundled ArgyllCMS's display system: `argyll_variant` (Wayland or
  X11) when chosen in Preferences, passed as `ARGYLL_WINDOW_SYSTEM` to every
  ArgyllCMS program (`argyll::set_window_system()`, a global read by
  `argyll::command()`, since all of them must agree; set at startup and
  when Preferences changes it). None, the default, sets nothing, leaving ArgyllCMS's own
  detection, which is the same one Preferences shows. An "Other" ArgyllCMS
  is never told.
- Preferences (`Prefs`, `Source`): a radio button per bundled variant from
  `system::current().variants()` (`Source::Bundled(window_system)`; on
  Linux "Bundled ArgyllCMS (auto)", the default, with the display system
  it detects shown beside it, then the two pinned ones; all disabled
  without a bundled ArgyllCMS), and "Other ArgyllCMS" with a directory
  (empty unless set; no placeholder, which would look like a default) and
  Browse. Each change rechecks with `argyll::probe()` (a
  `dispcal` that runs and reports a version) and
  `argyll::check_platform_as()` for that choice, showing "This session: …"
  and the supported / limited / unsupported line. The check runs on a
  thread (`Prefs::recheck()`/`poll()`; the probe's `dispcal -?` looks for
  instruments, which takes seconds), with a spinner and "Checking
  ArgyllCMS…" meanwhile, so the page opens at once; a newer check replaces
  a running one, whose result is dropped. A change that passes the probe
  takes effect when the check is in (`apply_when_checked`): saved (Auto as no `argyll_variant`),
  and displays and instruments scanned again; choosing the one in use
  rescans too. The directory field counts as a change when it loses focus
  with a different path. A failing choice isn't applied, and the error
  names the ArgyllCMS still in use. The controls are disabled during a
  run. A settings file with only `argyll_bin` (from before)
  loads as "Other". Browse
  uses rfd's XDG portal folder picker on its own thread, so the UI keeps
  drawing.
- Inventory comes from parsing `dispcal -?`: the version line, the `-d`
  display list (after ` -d n[,m] `, which is how every current Linux build
  of ArgyllCMS prints it, including the bundled one with or without X11, or
  ` -d n `, from older Wayland-only builds of the fork and from Windows and
  macOS builds, which "Other" may point at), the `-c` instrument list (entries without a parenthesised
  description are bare serial ports Argyll lists whether or not anything is
  attached, and are dropped), and the `-y` display types (either an
  instrument specific list on following lines, or the `Other: l = LCD, c =
  CRT` fallback).
- Instruments are shown by the name in parentheses (`instrument_name()`),
  except that Argyll's "X-Rite i1 DisplayPro, ColorMunki Display", the one
  instrument it lists under both names it's sold as, is shown as
  "i1Display Pro or ColorMunki Display". Commas aren't replaced in general,
  as some names have one ("Q, Inc Prisma").
- `Job::start` runs a tool with its output unbuffered
  (`System::live_output()`: on Linux under `stdbuf -o0 -e0`, which an
  ArgyllCMS other than the bundled one needs; elsewhere directly, as the
  bundled ArgyllCMS unbuffers its stdout itself when it isn't a terminal,
  and macOS and Windows have no stdbuf), with piped stdin, stdout
  and stderr, and turns its output into events: lines (split on `\r` and
  `\n`), `patch N of M` progress, and prompts (`Event::Prompt`, with the
  last four lines printed before it, which say what went wrong). Most
  prompts have no trailing newline, so the unterminated remainder is
  checked (`is_prompt()`: "Esc or Q" in any case, "any other key", or a
  trailing "continue:"), once it ends with a colon, as every prompt does
  (output can arrive in pieces: on Windows a prompt was split mid-word); complete lines are checked too, as "Hit any key to
  retry, or Esc or Q to abort:" after a failed instrument calibration ends
  with one. Argyll's `next_con_char`/`poll_con_char`
  `tcgetattr` warnings (stdin isn't a terminal) are printed without line
  breaks, so they're cut out of the middle of lines.
- `job_controls()` takes where Cancel and the status go. Measurement puts
  Cancel between the progress and the Log during a run, 1.5 rows from
  each, so opening the Log doesn't push it out of reach; Verify shows the status on its own
  line between the profile selector and Begin, and while measuring shows the
progress bar with Cancel under it, 1.5 rows apart; Report keeps both
inline.
- Prompts are answered on the page (`App::job_controls()`), not in a
  dialog. From Begin until the first prompt is answered (`placing`), the page
  asks for the instrument to be placed at the center of the screen, with
  Measure (enabled once Argyll has asked, and at least 5 s after the job
  started, `PLACING_TIME`, so the message can be read) and Cancel. Argyll prints "Place
  instrument on test window." as a complete line, so its placement prompt
  arrives as just the key instructions; with those removed it is shown as
  "With the spectro on the patch, press Measure to begin."
  `prompt_message()` turns a prompt into what's shown: after a misread, a
  sensor in the wrong position (an i1Display's diffuser over the lens) or
  a failed instrument calibration, "Unable to measure. Check that the
  device is uncovered and placed on the measurement patch, then press
  Measure to try again."; after a communication problem, a lost-contact
  message; otherwise the prompt's own text from before its key
  instructions, however they're worded. Measure writes a space to stdin,
  which retries or continues.
- Cancelling sends SIGTERM, so dispwin's signal handler restores the
  display's color settings. That handler exits with status 0, so the app
  records the cancel (`cancelled`) rather than trusting the exit status: a
  cancelled calibration deletes whatever `.cal`, `.icc` and `.ti3` it had
  written and the page returns to its start state with no message (a
  cancelled verification likewise), and a calibration only counts
  as done when both its `.cal` and `.icc` exist.

## Parameters (targets)

White point choices are D50, D55 and D65 (daylight 5000, 5500, 6500 K),
Native (no `-t`) and Custom, a daylight temperature typed in. Tonal
response is gamma 2.2, gamma 1.8, the sRGB curve or a custom gamma.
`Targets::kelvin()` and `Targets::gamma_arg()` give the `-t` and `-g`
values; `Targets::describe()` gives the "Parameters" line on the Measurement
page. Parameters shows the chosen display and its backlight correction
(correction file or display type) under the heading; Measurement shows the
instrument (the name from Argyll's description, without the USB path) and
correction. Its log is collapsed by default.

Profile type is Shaper+matrix (default) or LUT. Low quality can't be combined
with LUT: each greys out the other, and `Targets::settle()` raises a saved
Low+LUT to Medium at startup and when measuring starts. A test LUT from
Low's 100 patches verified at 0.96/2.95 against the matrix profile's
0.39/1.32 from the same patches. `targets::LOW_QUALITY_LUT` lifts the
restriction; it's set by building with the `low-quality-lut` feature
(`cargo run --features low-quality-lut`).

LUT has a circled question mark (`help()`, drawn rather than a font glyph)
explaining that it helps most with displays of poor linearity. On this
panel, profiles built from the same 250 measured patches (a Medium D55 run)
and verified against one measurement of the 57 patches through its
calibration: shaper+matrix average 0.38 / maximum 1.03 / grays 0.56; the
LUT as made 0.54 / 1.67 / 0.85; LUT at colprof `-qh` 0.58 / 1.93 / 1.01;
LUT with `-r 1.0` smoothing the same as made. The panel is linear enough
that a matrix describes it better than a table of its readings (it's a
factory-calibrated wide-gamut panel).

Verify tells the two cases apart (`verify::additivity()`): the largest
difference between white, yellow, cyan and magenta at full drive and the
sum of their channels measured alone (less the black in each reading), over
X, Y and Z, as a percentage of white's Y. It's shown in the summary card
("Channel additivity: off by N%") and the report. A shaper+matrix profile
(no `A2B0` tag, `profiles::Info::lut`) that doesn't verify Good gets advice:
over `verify::ADDITIVITY_LIMIT` (2%), that a LUT may verify better, with a
button that switches Parameters to LUT (raising Low to Medium) and opens them;
under it, that a LUT is unlikely to help. This panel is off by 0.26%. The
LUT help says to try Shaper+matrix first and that Verify will suggest a LUT
when it looks worthwhile. Parameters rows are spaced out
and wrap (`choices()`; `with_field()` keeps a Custom choice next to its
value).

The Spectro page lists instruments by name, with Argyll's full description
(the USB device) on hover. One dropdown chooses the correction: "None (use
Instrument Default)", the instrument's built-in display types (`-y`), or
correction files (`-X`), with Browse for others. Long entries are truncated
to the field's width and shown whole on hover. The display type choice isn't
saved; a correction file is.

## Measure

Making a profile is four jobs in a row (`JobKind::Calibrate`, `Targen`,
`ProfileRead`, `Colprof`), each started by `App::next_stage()` when the
previous one succeeds, sharing one log and shown as "Step N: <activity>"
above the progress bar (`App::stage_text()`, `JobKind::activity()`). The
step (`App::run_step`) is 1 for calibrating and goes up each time dispcal's
patch count starts over, which is its next calibration pass, then with
each later job, so a run with 3 passes shows Steps 1 to 6 (naming the
profile, between measuring and building it, isn't a numbered step). There's no "of
N": how many passes there are isn't known beforehand. Files are named
after `base`, in the data folder, `<output>-<YYYYMMDD-HHMMSS>`:

1. `dispcal -v -d D -c C [-y T|-X F] -m -q Q [-t K] -g G|s [-b B] -o
   work/<name>-quick.icc base` writes `base.cal`. `-m` skips the
   interactive monitor-controls adjustment. `-o` takes an optional file
   name, so it's always given; otherwise it would consume the base name
   that follows. dispcal's quick profile goes to `work/`, out of the
   profiles folder.
2. `targen -v -d3 -G -e4 -B4 -g9 -f N -c quick.icc base` writes `base.ti1`:
   video RGB, 4 white and 4 black patches, a 9 step gray axis, and the rest
   optimized spread patches placed using the quick profile, N in total
   (`Quality::patches()`: 100, 250 or 600; under 4 s even for 600).
3. `dispread -v -d D -c C [-y|-X] -Y p -k base.cal base` measures them
   through the new calibration, writing `base.ti3`; `-Y p` because the
   instrument is already in place. `-k` also records the calibration in the
   `.ti3`.
   The run's files are named `<output>-<stamp>` in the data folder until
   then. Measured, the page asks for a name (`CalRun::naming`,
   `naming_panel()`): "Save as", prefilled with the display's short name
   and the time ("eDP-1 2026-10-13 17:10"), with Save (or Enter) and
   Discard. `profile_stem()` makes the file name: path separators, colons
   (Finder shows them as slashes) and control characters become "-"; empty
   or hidden-file names aren't accepted, nor one already used by a profile
   in the data folder or installed (`name_problem()`). Save renames the
   `.cal`, `.ti1` and `.ti3` to it (`save_profile()`), then:
4. `colprof -v -q Q -a s|X -D name -M output -O name.icc name`:
   shaper+matrix (`s`) or an XYZ LUT with matrix tags as a fallback (`X`).
   The calibration in the `.ti3` becomes the profile's vcgt. The name is
   the profile's description, so the system's color settings show it; the
   model (dmdd) records the output it was made for. The Profiles page shows
   DisplayCraft profiles by file name and the description on hover.

KWin accepts both types: its ICC support reads BToA tables (lut8, lut16 and
lutBToA) and uses them in its color shader. A failed or cancelled stage
removes everything the run wrote (`CalRun::discard()`); a finished one
removes the quick profile. The result shows dispcal's final summary lines and
colprof's "Profile check complete, peak err = ..., avg err = ..." line.
"Install profile" runs `dispwin -d D -I base.icc`, which with the Wayland
ArgyllCMS installs it via ucmm and points KWin at the installed copy. A
successful install makes the new profile Verify's choice.

## Display arrangement

The background scan also runs `kscreen-doctor -j` (`layout::read()`):
each connected output's position, mode size, scale, rotation, priority
(1 = primary), enabled and `replicationSource` (mirroring). Positions are
logical, so sizes are divided by the scale and swapped for a quarter turn
(rotation 2 or 8). The Display page draws the enabled, unmirrored outputs to
fit 600 × 190 points (`display_map()`), each labelled with its name, model
(from Argyll's description), mode size and "primary", the selected one in
the accent colour. Each is matched to Argyll's display list by output name,
the same in both, or by `argyll_index` for outputs made from Argyll's
list; one Argyll doesn't list is marked and can't be chosen.
Clicking one selects it, the same selection as the dropdown below (and the
Profiles page). Off or mirroring outputs are listed under the map. The
rectangles are labelled for accessibility, which is also how tests click
them. Without kscreen-doctor (macOS, GNOME, other X11 desktops) the map
comes from Argyll's display list (`layout::from_argyll()`): each
description's "at X, Y, width W, height H" in desktop coordinates, which
also stand in for the size shown, "Primary" in it marking the primary,
the name from `display_name()`, and its place in the list kept in
`argyll_index`; descriptions that don't parse are left out. A monitor
plugged in later appears after Search Again.

To check with real monitors: plug them in and press Search Again on the Display
page; the map should match System Settings > Display Configuration
(positions, portrait monitors, primary); clicking each should select it in
the dropdown; Measure, Verify and Report should then put their patches on
that monitor; on Profiles, "In use", Use and Stop should act on it.

## Instrument access

`usbaccess::blocked()` looks through /sys/bus/usb/devices for devices
whose vendor and product IDs appear in ArgyllCMS's udev rule (a copy in
`assets/udev/55-Argyll.rules`, embedded), and tries opening each one's
/dev/bus/usb node for reading and writing; opening claims nothing. It runs
with the background scan. Any that can't be opened get a notice on the
Spectro page with "Install access rule…": the rule is written to `work/`
and `pkexec sh -c` installs it into /etc/udev/rules.d, reloads the rules
and triggers USB "add" events so devices already plugged in get access
(`usbaccess::install_command()`), then the scan runs again. On this machine
the instrument is reachable through colord's 69-cd-sensors.rules (group and
a session ACL), so nothing is shown; the notice was checked in a rendered
picture, and the install itself hasn't been run.

## Packaging

The build scripts (`appimage/build.sh`, `appimage/make-appimage.sh`,
`macos/make-app.sh`) take no settings from the environment: everything is
a command-line option with a default (`--src`, `--argyll-src`, `--cache`,
`--out`, `--work`, `--rust-home`, `--release-tag`; `build.sh`: `--argyll-src`,
`--out`, `--engine`), and the tools run in an environment the script
constructs. `make-appimage.sh` and `make-app.sh` start themselves again
under `env -i` (marked by a first argument `--in-clean-env`) with only
`HOME` (from the user database, `~user`, not `$HOME`), `PATH` (Rust's bin
and the system's directories), `CARGO_HOME`, `RUSTUP_HOME`, `TMPDIR` (in
the cache) and `GIT_CONFIG_GLOBAL` (a file in the cache, so neither the
user's git settings, ignore rules included, nor their `~/.gitconfig` are
involved). This matters for jam above all, which makes every environment
variable a build setting: run from one terminal session on the Mac, before
this, ArgyllCMS's build found 2 targets, built nothing and succeeded.
`build.sh` runs podman or docker under `env -i` with `HOME`, `PATH` and
`XDG_RUNTIME_DIR` (`/run/user/<uid>`, which rootless podman needs), and
passes the build its options. apt gets `DEBIAN_FRONTEND` on its command
line, since sudo drops the environment. The workflow passes `--cache`,
`--out` and, for a tag build, `--release-tag`.

The Wayland ArgyllCMS is the `argyllcms` submodule, https://github.com/bson/ArgyllCMS
(HTTPS in `.gitmodules` so anyone can clone it; the local checkout pushes
over SSH). `--argyll-src` builds from another checkout instead. A submodule's
git data is in DisplayCraft's `.git`, so in the container it's reached
through DisplayCraft's mount; another checkout gets a mount of its own.

`packaging/appimage/make-appimage.sh` does the build on Ubuntu 24.04
(glibc 2.39, the oldest base with Plasma 6.4+): `build.sh` runs it in a
podman or docker container, and the GitHub workflow runs it directly on an
ubuntu-24.04 runner. It uses `sudo` for apt only when not root, and the
rustup in `--rust-home` if there is one (the runners have it), otherwise
one kept in the cache. Given `--release-tag`, it stops unless the tag is
`v` plus Cargo.toml's version. The sources are mounted read-only and copied as git sees them
(tracked and new files, not ignored ones), so host build output, ArgyllCMS's
in-tree objects in particular, doesn't get linked in. ArgyllCMS is built
with `-sBUILTIN_TIFF/JPEG/PNG/Z/SSL=true`, as its own release script does,
leaving libwayland-client, libdbus and libX11 (with libxcb) as system
dependencies. The X extension libraries it also links (libXext, libXxf86vm,
libXinerama, libXrandr, libXrender, libXss), which a Wayland-only system
may lack, are bundled in `usr/lib/argyll/lib` and found through an
`$ORIGIN/../lib` rpath set with patchelf, not `LD_LIBRARY_PATH`, which
DisplayCraft would pass on to every program it starts. Its Wayland protocol
definitions come with it (`wlproto/` in the fork), as Ubuntu 24.04's are
too old. DisplayCraft is built with
rustup's stable Rust. The AppDir holds `usr/bin/displaycraft`, eleven
ArgyllCMS programs in `usr/lib/argyll/bin` (those DisplayCraft runs, plus
spotread, ccxxmake, oeminst and iccdump), the vendor corrections in
`usr/lib/argyll/ref` (found by `corrections::find()` beside the bin folder,
as for /opt/argyll-kde-wayland), icons, the desktop entry, licences, and
`ARGYLLCMS.txt`, which describes the modifications and gives the source
URL the AGPL calls for, with the commit built (and a warning if the
checkout had uncommitted changes). `AppRun` sets
`DISPLAYCRAFT_ARGYLL_BIN` to the bundled ArgyllCMS; a directory set in
Preferences still wins. appimagetool (from the AppImage project's releases)
runs extracted, as there's no FUSE in the container. Downloads are cached
in `tmp/appimage-cache`. The container runs with SELinux labelling off
(`--security-opt label=disable`) rather than relabelling mounts with `:Z`,
which would relabel the source trees.

`packaging/ARGYLLCMS.txt` is shared by both packages.

`packaging/macos/make-app.sh` builds on macOS itself (a GitHub macos-15
runner, Apple Silicon, or a Mac with Xcode's command line tools and Rust),
for the machine's architecture, with `MACOSX_DEPLOYMENT_TARGET=11.0` for
rustc and ArgyllCMS's compiles alike. Neither Homebrew nor Xcode has jam,
so it builds ArgyllCMS's own Jam 2.5 (`ajam-2.5.2-1.3.3.tgz` from
argyllcms.com, checked by SHA-256), only its first stage `jam0`, with
warnings current clang treats as errors turned off; it's cached. Sources
are copied as for the AppImage, ArgyllCMS built with the same
`-sBUILTIN_*` options (so nothing comes from Homebrew; the clean
environment keeps Homebrew's directories off `PATH` too), stopping if it
installed no `dispcal`, since jam can succeed having built nothing,
DisplayCraft with `cargo --locked`. `DisplayCraft.app`: `Contents/MacOS/displaycraft`, the
same eleven ArgyllCMS programs in `Contents/Resources/argyll/bin` and the
corrections in `.../argyll/ref` (`system::MacOs::bundled_dir()` looks
there, beside the executable, unless `DISPLAYCRAFT_ARGYLL_BIN` is set),
licences and `ARGYLLCMS.txt` (with the commit) in `Resources/doc`, an
`.icns` made with iconutil from the hicolor PNGs (no 1024 px, so no
512@2x), and an `Info.plist` (`io.github.bson.displaycraft`, version
from Cargo.toml). The script fails if `otool -L` shows anything outside
`/usr/lib` and `/System/Library`. strip removes the linker's signature,
so the bundle gets an ad-hoc one (`codesign --sign -`), without which
Apple Silicon won't run it; it's no Developer ID, so Gatekeeper still
asks. It goes in a disk image, `DisplayCraft-<version>-macos-<arch>.dmg`
(`hdiutil`, UDZO, with an Applications link to drag it to), which keeps
its signature and file modes as one file.

`packaging/windows/make-app.ps1` builds on Windows itself (a GitHub
windows-2025 runner, or a machine with rustup; developed in a WinBoat VM).
Its options are PowerShell parameters with defaults (`-Src`,
`-ArgyllSrc`, `-Cache`, `-Out`, `-Work`, `-RustHome`, `-ReleaseTag`), and
every tool runs through `Invoke-Tool`, a `ProcessStartInfo` whose
environment is cleared and refilled from Windows' own folder lookups
(`[Environment]::GetFolderPath`), the cache's temp folder, and what each
tool needs (Path, `MINGW`, `CARGO_HOME`, `RUSTUP_HOME`,
`GIT_CONFIG_GLOBAL`), so nothing comes from the calling shell. Into the
cache, checked by SHA-256: WinLibs' MinGW-w64 (GCC 16.2, UCRT) and
ArgyllCMS's jam (`ajam-2.5.2-1.3.4.zip`, built `gcc -DNT -fpermissive`),
and Inno Setup 7.1.0, installed for the current user (`/CURRENTUSER`).
Rust's `stable-x86_64-pc-windows-gnu` toolchain comes from the rustup in
`-RustHome` (or one installed in the cache). The sources are copied with
robocopy, leaving out `.git` and, for DisplayCraft only, `target`, `tmp`,
`dist` and the submodule (ArgyllCMS has a `target` source folder);
ArgyllCMS builds with the usual `-sBUILTIN_*` options, `MINGW` and a
relative `DESTDIR`; DisplayCraft with `cargo +stable-x86_64-pc-windows-gnu
build --release --locked`. The folder has `displaycraft.exe`,
`displaycraft.ico`, the eleven ArgyllCMS programs in `argyll\bin`
(`Single::bundled_dir()`), `argyll\ref`, and licences and `ARGYLLCMS.txt`
with the commit in `doc`. The script fails if any program imports a DLL
that isn't in System32 (objdump -p). Out of it come
`DisplayCraft-<version>-windows-x64.zip` and, from
`packaging/windows/DisplayCraft.iss`, the installer
`DisplayCraft-<version>-windows-x64-setup.exe`: per-user by default
(`PrivilegesRequired=lowest`, `{autopf}`, so `%LOCALAPPDATA%\Programs`;
an administrator can choose all users), a Start menu entry, an
uninstaller, and a fixed `AppId` so new versions replace old ones.
`assets/displaycraft.ico` (16 to 256 px) was made from the hicolor PNGs
with ImageMagick.

`.github/workflows/build.yml`: `appimage`, `macos` and `windows` jobs each
build and upload their files as artifacts (two weeks; one upload step
per file, as unzipped uploads take one each); "Run workflow" on the
Actions tab stops there. On a `v*` tag, a `release` job that needs all of them
downloads them and publishes a GitHub release with all of them
(`gh release create`, with generated notes), so a release only appears
when every platform built. If a release for the tag already exists, the
job leaves it as it is.

Releases: a test build's files can be published once they've been tried,
so the release is exactly what was tested (test files are kept 14 days):

    gh run download <run id> --repo bson/displaycraft -D tmp/release
    gh release create v<version> tmp/release/* --repo bson/displaycraft \
        --target <the run's commit> --title "DisplayCraft <version>" --generate-notes

Creating the release creates and pushes the tag, which runs the workflow
for it; its release job then finds the release and keeps it. The tag
must be `v` plus Cargo.toml's version, which the tag build checks. A tag
names a commit whose workflow has this check, so a run made before the
check was added is released from a later commit with the same code. Each job caches its build output keyed on
`Cargo.lock` (and appimagetool or jam). Uploads use
`upload-artifact@v7` with `archive: false`, so the AppImage and the disk
image download as they are (the artifact takes the file's name), and the
release job's `download-artifact@v8` passes them through unzipped.

Built so far: 17.5 MB, needing glibc 2.39 at most; the bundled ArgyllCMS
programs need only libwayland-client and libdbus. Started here with fresh
settings, it used its own ArgyllCMS (`dispwin -Q` reports the platform as
supported), listed the panel and drew the arrangement. Not yet run on
another distribution.

## Adjusting the display's controls

Parameters has an Adjust… button (Display controls). It runs `dispcal -v -d D
-c C [-y|-X] [-t K] [-b B] work/adjust` without `-m`, a job of its own
(`JobKind::Adjust`), and leaves with Exit (8), so nothing is written and
calibrating stays the Measure step's. After placing the instrument, dispcal
shows a menu ("Press 1 .. 7"); `adjust::State` follows it from the output
lines, and the page offers White point (2), White level (3), Black level
(1) and Black point (4) as buttons, and Finish (8). In a mode, dispcal
prints an instruction line and a target ("Target Br ..., x ..., y ...", or
"Initial ..." when native), then rewrites "Current ..." in place about once
a second until space. Those readings end only with the next "\r", so the
reader passes the unterminated text on as `Event::Live` and drops it, which
also keeps them out of the log. The panel shows the level, chromaticity
with arrows, ΔE (coloured), and which way to turn red, green and blue, the
one to turn first (dispcal's doubled sign) underlined; Done sends space.
The Display controls row (label and Adjust…) sits under the Display line,
above the targets, and the placement prompt, panel and Cancel appear right
under it, so they stay in view; the page scrolls, and the panel scrolls
itself into view when it changes between the menu and an adjustment. With
the panel at the foot of the page, the placement prompt's Measure button
was scrolled off at the default window height, where egui ignores clicks,
and dispcal waited at its prompt. The targets are disabled while it runs.

A `Job` dropped while its tool runs is cancelled (SIGTERM). Before that, a
test that panicked mid-adjustment closed dispcal's input, and dispcal
exited without restoring KWin's sRGB mode and accuracy preference.

## Graphs

`profiles::curves()` reads the vcgt (table or formula), the TRCs (`curv` or
`para`) and the colorants, sampled at 256 points. The colorants are adapted
to D50: ICC v4's `chad` is the adaptation; Argyll's `arts` is the cone space
the adaptation was done in (not the adaptation), so it's rebuilt with the
v2 absolute white point; plain Bradford otherwise. For this panel's
profiles the primaries came out within 0.001 of the Report's measurements.
The Profiles page shows a profile's calibration curves, tonal response (with
gamma 2.2 dashed) and gamut when its name is clicked; the Report page shows
the measured gamut beside the figures. The spectral locus (380-700 nm, 5 nm)
was computed from the CIE 1931 2° colour matching functions in ArgyllCMS's
`xicc/xspect.c`.

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
6. `verify::html()` writes a self-contained HTML report with SVG charts
   (ΔE per patch in the verdict colours with the 3 and 5 limits, gray
   balance from the gray ramp's measured a* and b*, and the profile's gamut,
   calibration and tonal response), the figures and the patch table with
   swatches; it prints cleanly. Open shows it. Both reports and the summary
   card name the profile type (shaper+matrix or LUT, from its `A2B0` tag).
7. Files go to `$XDG_DATA_HOME/displaycraft/verify/<profile>-verify-<stamp>`
   (`-expected.ti1/.ti3`, `-measured.ti1/.ti3`, `.txt` and `.html`
   reports).

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
The page opens with a display dropdown; it shares the selection with the
Display page, which uses a dropdown too (with Argyll's position and size
under it). This display's profiles come first. A DisplayCraft profile
records the output it was made for in its model (dmdd; read only for
profiles in the data folder, as vendor profiles put other text there), or
before naming existed, in its name, `<output>-<YYYYMMDD>-<HHMMSS>.icc`
(`app::made_for()`). One made for another output is marked "made for X",
and Use warns on hover; other profiles carry no such mark. What's in use comes from `kscreen-doctor -o` (its JSON leaves out the
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
  egui_kittest: Measure, Begin, then Measure or Continue at each Argyll prompt,
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
  renders Display, Spectro, Parameters, Measurement and Verify, with made-up
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
- With only the laptop panel here, multiple displays were checked in the
  rendered pictures with a made-up DP-3 added: the Display dropdown lists
  both, and choosing DP-3 on Profiles moved the unmarked profile first and
  marked the eDP-1 ones "made for eDP-1".
- The GitHub test build (AppImage of 9826341), downloaded on another
  desktop with several displays, ran, offered display selection, and
  calibrated a display other than the one DisplayCraft's window was on.

- `app::hardware_tests::calibrate_and_profile` (ignored by default; Low
  quality, saved Parameters, `DISPLAYCRAFT_TEST_LUT=1` for LUT) runs the four
  stages through the page without installing. D50, gamma 2.2, 100 patches:
  - Shaper+matrix: 603 s; calibration white point error 0.48 dE, average
    neutral 0.43; colprof check average 0.39, peak 1.23. Verified Good:
    average 0.39, maximum 1.32, grays 0.52/1.28 (the dispcal -o profile it
    replaces verified 0.71/1.96). The profile has a vcgt; the work folder
    was emptied.
  - LUT: 563 s; colprof check average 1.80, peak 6.23. Verified Good but
    weaker: average 0.96, maximum 2.95. 100 patches are too few for a LUT.
    Installed briefly, KWin took it in ICC mode with no warnings.

- `app::hardware_adjust_tests::adjust_through_settings` (ignored by
  default) runs the adjustment from Parameters on the real panel: placement,
  the menu, White point with live readings (target 117.7 cd/m², x 0.3325,
  y 0.3476 for D55; reading x 0.3124, y 0.3430, ΔE 10.4, "Red up" first,
  as expected for native white near 6600 K), Done, Finish; KWin was back in
  ICC mode with its accuracy preference afterwards. The picture is
  `tmp/page-adjusting.png`.
- A real verification's HTML report rendered in headless Chrome: charts and
  table as intended.

- The arrangement map, rendered with a made-up layout (the panel at 1.25
  scale, a portrait DP-3 above and to its right, an HDMI output switched
  off): placed as arranged, HDMI listed as off; clicking eDP-1 and DP-3 in
  the map selected each.

Not yet tested by hand: the Preferences page (Browse, changing the choice), and
the correction list and Browse on the Spectro page, Export CSV, and the
Measure and Cancel buttons while measuring, the Profiles page's buttons
(clicked), and deleting a profile.
