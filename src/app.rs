//! The calibration workflow: pick a display and instrument, set targets, then
//! run dispcal to calibrate and make a matrix profile, and install it.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use egui::{Align, Layout, RichText, ScrollArea};

use crate::adjust::{self, Dir};
use crate::argyll::{self, Event, Inventory, Job, Platform, Session, WindowSystem};
use crate::charts;
use crate::corrections::{self, Correction};
use crate::layout::{self, display_name};
use crate::profiles;
use crate::report;
use crate::settings::Settings;
use crate::system;
use crate::targets::{ProfileType, Quality, Targets, Tone, White};
use crate::usbaccess;
use crate::verify;
use crate::theme::{self, ThemeKind, Tokens};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Display,
    Instrument,
    Targets,
    Run,
    Verify,
    Report,
    Profiles,
    /// At the foot of the sidebar, and open even where ArgyllCMS can't run
    Preferences,
    About,
}

impl Step {
    /// The calibration sequence, numbered in the sidebar
    const WORKFLOW: [Step; 5] = [Step::Display, Step::Instrument, Step::Targets, Step::Run, Step::Verify];
    /// Pages outside the sequence, listed below it
    const TOOLS: [Step; 2] = [Step::Report, Step::Profiles];

    fn label(self) -> &'static str {
        match self {
            Step::Display => "Display",
            Step::Instrument => "Spectro",
            Step::Targets => "Settings",
            Step::Run => "Measure",
            Step::Verify => "Verify",
            Step::Report => "Report",
            Step::Profiles => "Profiles",
            Step::Preferences => "Preferences",
            Step::About => "About",
        }
    }

    /// Pages that open even where ArgyllCMS can't run, without its warnings
    fn always_open(self) -> bool {
        matches!(self, Step::Preferences | Step::About)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    /// dispcal, the first of the four stages that make a profile
    Calibrate,
    /// dispcal's interactive adjustment of the display's controls, from the
    /// Settings page
    Adjust,
    /// targen: choose the profile patches
    Targen,
    /// dispread: measure them through the new calibration
    ProfileRead,
    /// colprof: build the profile
    Colprof,
    Install,
    Verify,
    Report,
    /// Install a profile from the Profiles page
    UseProfile,
    /// Uninstall the display's profile, back to sRGB
    StopProfile,
    /// Install the instrument access rule, with pkexec
    InstallRule,
}

impl JobKind {
    /// Where a profile-making job is in the sequence, for the page
    fn stage(self) -> Option<&'static str> {
        match self {
            JobKind::Calibrate => Some("Step 1 of 4: calibrating"),
            JobKind::Targen => Some("Step 2 of 4: choosing the profile patches"),
            JobKind::ProfileRead => Some("Step 3 of 4: measuring the profile patches"),
            JobKind::Colprof => Some("Step 4 of 4: building the profile"),
            _ => None,
        }
    }

    /// The page that shows this job
    fn step(self) -> Step {
        match self {
            JobKind::Calibrate | JobKind::Targen | JobKind::ProfileRead | JobKind::Colprof | JobKind::Install => Step::Run,
            JobKind::Verify => Step::Verify,
            JobKind::Report => Step::Report,
            JobKind::Adjust => Step::Targets,
            JobKind::InstallRule => Step::Instrument,
            JobKind::UseProfile | JobKind::StopProfile => Step::Profiles,
        }
    }
}

/// A verification in progress: where its files go.
struct VerifyRun {
    profile: PathBuf,
    /// Base name for the files, e.g. ".../verify/eDP-1-...-verify-20261012-190000"
    base: PathBuf,
    correction: Option<PathBuf>,
}

/// A finished verification.
struct VerifyOutcome {
    profile: PathBuf,
    /// The profile is a LUT rather than shaper+matrix
    lut: bool,
    /// `verify::additivity()` of the measurement
    additivity: Option<f64>,
    summary: verify::Summary,
    results: Vec<verify::PatchResult>,
    report: PathBuf,
}

/// A calibration and profile in the making. Each stage's files are named
/// after `base`: dispcal writes `.cal`, targen `.ti1`, dispread `.ti3` and
/// colprof `.icc`.
struct CalRun {
    base: PathBuf,
    /// dispcal's quick profile, which targen uses to spread the patches over
    /// what the display can show; kept out of the profiles folder
    quick: PathBuf,
    /// dispcal's summary of how close the calibration got
    summary: Vec<String>,
    /// The new profile's description
    description: String,
}

impl CalRun {
    /// Remove what an unfinished run left behind
    fn discard(&self) {
        for ext in ["cal", "ti1", "ti3", "icc"] {
            let _ = std::fs::remove_file(self.base.with_extension(ext));
        }
        let _ = std::fs::remove_file(&self.quick);
    }
}

/// What the last calibration produced.
struct Outcome {
    cal: PathBuf,
    icc: PathBuf,
    /// dispcal's own summary of how close it got, e.g. "White point error = 0.35 deltaE"
    summary: Vec<String>,
    installed: bool,
}

/// Which ArgyllCMS Preferences has chosen
#[derive(Clone, Copy, PartialEq)]
enum Source {
    /// The bundled one, told to use this display system, or with None,
    /// left to choose (see system::Variant)
    Bundled(Option<WindowSystem>),
    Other,
}

/// The Preferences window's unapplied state.
struct Prefs {
    source: Source,
    /// The other ArgyllCMS's directory
    path: String,
    /// The last check of the choice: ArgyllCMS version, or what's wrong.
    check: Option<Result<String, String>>,
    /// And what that ArgyllCMS made of this session
    session: Option<Session>,
    /// A folder picker running on its own thread.
    picker: Option<Receiver<Option<PathBuf>>>,
}

impl Prefs {
    fn from_settings(settings: &Settings) -> Prefs {
        let mut prefs = Prefs {
            source: match &settings.argyll_bin {
                Some(_) => Source::Other,
                None => Source::Bundled(settings.argyll_variant),
            },
            path: settings.argyll_bin.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            check: None,
            session: None,
            picker: None,
        };
        prefs.recheck();
        prefs
    }

    fn other_dir(&self) -> Option<PathBuf> {
        let path = self.path.trim();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    /// The directory the choice amounts to
    fn dir(&self) -> Option<PathBuf> {
        match self.source {
            Source::Bundled(_) => system::current().bundled_dir(),
            Source::Other => self.other_dir(),
        }
    }

    /// Check the chosen ArgyllCMS, and ask it about the session.
    fn recheck(&mut self) {
        let Some(dir) = self.dir() else {
            self.check = Some(Err(match self.source {
                Source::Bundled(_) => system::current().no_bundled().into(),
                Source::Other => "Choose the directory holding the ArgyllCMS programs".into(),
            }));
            self.session = None;
            return;
        };
        self.check = Some(argyll::probe(&dir));
        // Another ArgyllCMS is left to choose for itself, like Auto
        let session = argyll::check_platform_as(&dir, self.variant());
        self.session = Some(session);
    }

    /// The bundled variant to pin, or None to follow detection
    fn variant(&self) -> Option<WindowSystem> {
        match self.source {
            Source::Bundled(v) => v,
            Source::Other => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ReportMode {
    /// The display's own response, through a linear calibration
    Uncalibrated,
    /// Through whatever calibration is loaded
    Current,
}

impl ReportMode {
    fn title(self) -> &'static str {
        match self {
            ReportMode::Uncalibrated => "uncalibrated",
            ReportMode::Current => "as calibrated now",
        }
    }
}

/// A report in progress: its base file name and what it measures
struct ReportRun {
    base: PathBuf,
    mode: ReportMode,
    output: String,
}

struct ReportOutcome {
    report: report::Report,
    title: String,
    saved: PathBuf,
}

/// One profile on the Profiles page. A DisplayCraft profile and its
/// installed copy share a file name, so they're one row.
struct ProfileRow {
    name: String,
    info: profiles::Info,
    /// DisplayCraft's own copy
    made: Option<PathBuf>,
    /// The copy installed for displays
    installed: Option<PathBuf>,
    verdict: Option<String>,
    /// The output it was made for, when DisplayCraft made it
    made_for: Option<String>,
    /// For its graphs
    curves: Option<profiles::Curves>,
}

impl ProfileRow {
    fn path(&self) -> &std::path::Path {
        self.installed.as_deref().or(self.made.as_deref()).expect("a row has at least one copy")
    }
}

/// What the Profiles page shows, read when it's opened and after changes
struct ProfilesView {
    output: String,
    color: Option<profiles::OutputColor>,
    rows: Vec<ProfileRow>,
}

/// What a background scan found
struct Scan {
    session: Session,
    inventory: Result<Inventory, String>,
    /// KDE's arrangement of the outputs, if kscreen-doctor answered
    layout: Vec<layout::Output>,
    /// Instruments plugged in that the user can't open
    blocked: Vec<usbaccess::Blocked>,
}

pub struct App {
    ctx: egui::Context,
    /// A scan for displays and instruments in progress
    scan: Option<Receiver<Scan>>,
    /// How KDE arranges the outputs, for the Display page's map
    layout: Vec<layout::Output>,
    /// Instruments plugged in that the user can't open
    blocked: Vec<usbaccess::Blocked>,
    settings: Settings,
    prefs: Option<Prefs>,
    bin: Option<PathBuf>,
    /// What ArgyllCMS says about this session (None until it's been found)
    /// What ArgyllCMS made of this session, once asked
    session: Option<Session>,
    inventory: Inventory,
    inventory_error: Option<String>,
    theme: ThemeKind,
    step: Step,
    display: usize,
    instrument: usize,
    display_type: Option<usize>,
    job: Option<(JobKind, Job)>,
    log: Vec<String>,
    progress: Option<(u32, u32)>,
    prompt: Option<String>,
    /// From Begin until the user says the instrument is in place: the first
    /// prompt (Argyll's "Place instrument on test window") is answered by
    /// the Measure button rather than shown
    placing: bool,
    /// Cancel was clicked for the running job. Argyll exits with status 0
    /// when signalled, so this is how a cancelled run is told from a
    /// finished one.
    cancelled: bool,
    status: Option<(bool, String)>,
    cal_run: Option<CalRun>,
    /// dispcal's interactive adjustment, while it runs
    adjust: adjust::State,
    /// What the adjustment panel last showed (menu, or a mode), so it's
    /// scrolled into view when that changes
    adjust_shown: Option<Option<adjust::Mode>>,
    outcome: Option<Outcome>,
    corrections: Vec<Correction>,
    /// A file chosen with Browse, picked on its own thread
    correction_picker: Option<Receiver<Option<PathBuf>>>,
    verify_profile: Option<PathBuf>,
    verify_picker: Option<Receiver<Option<PathBuf>>>,
    verify_run: Option<VerifyRun>,
    verify_outcome: Option<VerifyOutcome>,
    /// Where to export the verification CSV, picked on its own thread
    csv_picker: Option<Receiver<Option<PathBuf>>>,
    /// Height of the Verify page's log section last frame, open or closed
    verify_log_height: f32,
    report_mode: ReportMode,
    report_run: Option<ReportRun>,
    report_outcome: Option<ReportOutcome>,
    profiles_view: Option<ProfilesView>,
    /// A DisplayCraft profile waiting for the user to confirm deleting it
    delete_confirm: Option<PathBuf>,
    /// The profile whose graphs the Profiles page shows, by file name
    profile_details: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings = Settings::load();
        let theme = settings
            .theme
            .as_deref()
            .and_then(|label| ThemeKind::ALL.into_iter().find(|k| k.label() == label))
            .unwrap_or_default();
        theme::apply_kind(&cc.egui_ctx, theme);
        let mut settings = settings;
        settings.targets.settle();
        Self::tell_window_system(&settings);
        let mut app = App {
            ctx: cc.egui_ctx.clone(),
            scan: None,
            layout: Vec::new(),
            blocked: Vec::new(),
            bin: argyll::find_bin_dir(settings.argyll_bin.as_deref()),
            settings,
            prefs: None,
            session: None,
            inventory: Inventory::default(),
            inventory_error: None,
            theme,
            step: Step::Display,
            display: 0,
            instrument: 0,
            display_type: None,
            job: None,
            log: Vec::new(),
            progress: None,
            prompt: None,
            placing: false,
            cancelled: false,
            status: None,
            cal_run: None,
            adjust: adjust::State::default(),
            adjust_shown: None,
            outcome: None,
            corrections: Vec::new(),
            correction_picker: None,
            verify_profile: None,
            verify_picker: None,
            verify_run: None,
            verify_outcome: None,
            csv_picker: None,
            verify_log_height: 30.0,
            report_mode: ReportMode::Uncalibrated,
            report_run: None,
            report_outcome: None,
            profiles_view: None,
            delete_confirm: None,
            profile_details: None,
        };
        app.corrections = corrections::find(app.bin.as_deref());
        app.refresh_inventory();
        app.verify_profile = app.saved_profiles().into_iter().next();
        app
    }

    /// The bundled ArgyllCMS's display system, if one was chosen in
    /// Preferences. Otherwise ArgyllCMS's own detection, which is what
    /// DisplayCraft would have chosen too.
    fn tell_window_system(settings: &Settings) {
        argyll::set_window_system(if settings.argyll_bin.is_none() { settings.argyll_variant } else { None });
    }

    /// Identify what can be measured: first whether ArgyllCMS supports this
    /// session at all, then the displays and instruments.
    fn refresh_inventory(&mut self) {
        let Some(bin) = self.bin.clone() else {
            self.scan = None;
            self.session = None;
            self.inventory = Inventory::default();
            self.inventory_error = Some("ArgyllCMS wasn't found. Set its directory in Preferences.".into());
            return;
        };
        // dispcal -? looks for instruments, which takes seconds, so it runs
        // on its own thread and the window opens at once. A scan started
        // while another runs replaces it; the old one's result is dropped.
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let session = argyll::check_platform(&bin);
            let inventory = match session.platform {
                Platform::Unsupported(_) => Ok(Inventory::default()),
                _ => argyll::inventory(&bin).map_err(|e| {
                    format!("Couldn't run dispcal in {}: {e}. Check the ArgyllCMS directory in Preferences.", bin.display())
                }),
            };
            // KDE's arrangement where there is one, which has more to show;
            // otherwise ArgyllCMS's display list gives positions and sizes
            let layout = layout::read().unwrap_or_else(|| {
                let descriptions: Vec<String> = inventory.as_ref().map(|i| i.displays.iter().map(|d| d.description.clone()).collect()).unwrap_or_default();
                layout::from_argyll(&descriptions)
            });
            let blocked = usbaccess::blocked();
            let _ = tx.send(Scan { session, inventory, layout, blocked });
            ctx.request_repaint();
        });
        self.scan = Some(rx);
    }

    /// Take a finished scan's result
    fn poll_scan(&mut self) {
        let Some(rx) = &self.scan else { return };
        let Ok(scan) = rx.try_recv() else { return };
        self.scan = None;
        self.session = Some(scan.session);
        self.layout = scan.layout;
        self.blocked = scan.blocked;
        match scan.inventory {
            Ok(inv) => {
                self.display = self.display.min(inv.displays.len().saturating_sub(1));
                self.instrument = self.instrument.min(inv.instruments.len().saturating_sub(1));
                self.display_type = self.display_type.filter(|&i| i < inv.display_types.len());
                self.inventory = inv;
                self.inventory_error = None;
            }
            Err(e) => {
                self.inventory = Inventory::default();
                self.inventory_error = Some(e);
            }
        }
    }

    fn unsupported(&self) -> Option<&str> {
        match self.session.as_ref().map(|s| &s.platform) {
            Some(Platform::Unsupported(why)) => Some(why),
            _ => None,
        }
    }

    fn ready(&self) -> Result<(), &'static str> {
        if self.scan.is_some() {
            Err("Looking for displays and spectros…")
        } else if self.unsupported().is_some() {
            Err("ArgyllCMS can't run on this platform")
        } else if self.inventory.displays.is_empty() {
            Err("No display found")
        } else if self.inventory.instruments.is_empty() {
            Err("No instrument found. Connect one and refresh.")
        } else {
            Ok(())
        }
    }

    /// Where results go: $XDG_DATA_HOME/displaycraft, named after the output.
    fn output_base(&self) -> std::io::Result<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| std::io::Error::other("no HOME"))?;
        let dir = data.join("displaycraft");
        std::fs::create_dir_all(&dir)?;
        let output = self.inventory.displays.get(self.display).map(|d| output_name(&d.description)).unwrap_or_else(|| "display".into());
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        Ok(dir.join(format!("{output}-{stamp}")))
    }

    fn start_calibration(&mut self, ctx: &egui::Context) {
        let Some(bin) = self.bin.clone() else { return };
        let base = match self.output_base() {
            Ok(b) => b,
            Err(e) => {
                self.status = Some((false, format!("Can't create the output directory: {e}")));
                return;
            }
        };
        // Settings can't make a disallowed combination, but saved ones might
        self.settings.targets.settle();
        let display = &self.inventory.displays[self.display];
        let t = &self.settings.targets;
        let mut args: Vec<String> = vec!["-v".into(), "-d".into(), display.value.clone()];
        args.extend(self.instrument_args());
        // Skip the interactive adjustment, which Settings offers on its own
        args.push("-m".into());
        args.extend(["-q".into(), t.quality.arg().into()]);
        if let Some(k) = t.kelvin() {
            args.extend(["-t".into(), format!("{k:.0}")]);
        }
        args.extend(["-g".into(), t.gamma_arg()]);
        if t.set_brightness {
            args.extend(["-b".into(), format!("{:.0}", t.brightness)]);
        }
        let work = match Self::data_dir().map(|d| d.join("work")) {
            Ok(d) if std::fs::create_dir_all(&d).is_ok() => d,
            _ => {
                self.status = Some((false, "Can't create the work directory".into()));
                return;
            }
        };
        let name = base.file_name().unwrap().to_string_lossy().into_owned();
        let quick = work.join(format!("{name}-quick.icc"));
        // -o takes an optional file name, so give it one: otherwise it
        // would take the output base name that follows
        args.extend(["-o".into(), quick.display().to_string(), base.display().to_string()]);

        let output = self.inventory.displays.get(self.display).map(|d| output_name(&d.description)).unwrap_or_default();
        let description = format!("{output} {}, {}", chrono::Local::now().format("%Y-%m-%d %H:%M"), t.describe());
        self.start_job(ctx, JobKind::Calibrate, &bin, "dispcal", &args);
        self.cal_run = Some(CalRun { base, quick, summary: Vec::new(), description });
        self.outcome = None;
    }

    /// Start the stage after `done`, once it has succeeded
    fn next_stage(&mut self, ctx: &egui::Context, done: JobKind) {
        let (Some(bin), Some(run)) = (self.bin.clone(), self.cal_run.as_ref()) else { return };
        let t = &self.settings.targets;
        let base = run.base.display().to_string();
        match done {
            JobKind::Calibrate => {
                // Video RGB; white and black patches, a gray axis, the rest
                // spread out (-G: optimized rather than fast), all to the
                // quality's total
                let mut args: Vec<String> = ["-v", "-d3", "-G", "-e4", "-B4", "-g9", "-f"].map(String::from).to_vec();
                args.push(t.quality.patches().to_string());
                if run.quick.is_file() {
                    args.extend(["-c".into(), run.quick.display().to_string()]);
                }
                args.push(base);
                self.start_job(ctx, JobKind::Targen, &bin, "targen", &args);
            }
            JobKind::Targen => {
                let display = &self.inventory.displays[self.display];
                let mut args: Vec<String> = vec!["-v".into(), "-d".into(), display.value.clone()];
                args.extend(self.instrument_args());
                // The instrument is already in place from calibrating
                args.extend(["-Y".into(), "p".into()]);
                args.extend(["-k".into(), run.base.with_extension("cal").display().to_string(), base]);
                self.start_job(ctx, JobKind::ProfileRead, &bin, "dispread", &args);
            }
            JobKind::ProfileRead => {
                // dispread -k put the calibration in the .ti3, so colprof
                // embeds it as the profile's vcgt
                let args: Vec<String> = vec![
                    "-v".into(),
                    "-q".into(),
                    t.quality.arg().into(),
                    "-a".into(),
                    t.profile.arg().into(),
                    "-D".into(),
                    run.description.clone(),
                    "-O".into(),
                    run.base.with_extension("icc").display().to_string(),
                    base,
                ];
                self.start_job(ctx, JobKind::Colprof, &bin, "colprof", &args);
            }
            _ => {}
        }
    }

    /// The chosen display, e.g. "eDP-1 (Sharp Corporation eDP-1)"
    fn display_label(&self) -> Option<String> {
        self.inventory.displays.get(self.display).map(|d| display_name(&d.description))
    }

    /// The correction for the display's backlight: a correction file, a
    /// display type, or None for the instrument's default
    fn correction_label(&self) -> Option<String> {
        if let Some(path) = &self.settings.correction {
            return Some(match self.corrections.iter().find(|c| &c.path == path) {
                Some(c) => c.label(),
                None => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            });
        }
        self.display_type.and_then(|i| self.inventory.display_types.get(i)).map(|dt| dt.description.clone())
    }

    fn looking(ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Looking for displays and spectros…");
        });
    }

    /// Space between a page's parts: one and a half text rows
    fn gap(ui: &mut egui::Ui) {
        let row = ui.text_style_height(&egui::TextStyle::Body);
        ui.add_space(row * 1.5);
    }

    /// A dim line of context under a page's heading
    fn context_line(ui: &mut egui::Ui, text: String) {
        let t = Tokens::get(ui.ctx());
        ui.label(RichText::new(text).color(t.text_dim));
    }

    /// The instrument and its correction: a correction file replaces the
    /// display type, which selects one of the instrument's built-in ones.
    fn instrument_args(&self) -> Vec<String> {
        let mut args = vec!["-c".to_string(), self.inventory.instruments[self.instrument].value.clone()];
        if let Some(c) = &self.settings.correction {
            args.extend(["-X".into(), c.display().to_string()]);
        } else if let Some(dt) = self.display_type.and_then(|i| self.inventory.display_types.get(i)) {
            args.extend(["-y".into(), dt.value.clone()]);
        }
        args
    }

    fn data_dir() -> std::io::Result<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| std::io::Error::other("no HOME"))?;
        Ok(data.join("displaycraft"))
    }

    /// Profiles DisplayCraft made, newest first.
    fn saved_profiles(&self) -> Vec<PathBuf> {
        let Ok(dir) = Self::data_dir() else { return Vec::new() };
        let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
            .map(|it| {
                it.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "icc"))
                    .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
                    .collect()
            })
            .unwrap_or_default();
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.into_iter().map(|(_, p)| p).collect()
    }

    fn start_verification(&mut self, ctx: &egui::Context) {
        let (Some(bin), Some(profile)) = (self.bin.clone(), self.verify_profile.clone()) else { return };
        let fail = |app: &mut App, why: String| app.status = Some((false, why));
        let dir = match Self::data_dir().map(|d| d.join("verify")) {
            Ok(d) => d,
            Err(e) => return fail(self, format!("No place for the results: {e}")),
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return fail(self, format!("Can't create {}: {e}", dir.display()));
        }
        let stem = profile.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "profile".into());
        let base = dir.join(format!("{stem}-verify-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        let expected = base.with_file_name(format!("{}-expected", base.file_name().unwrap().to_string_lossy()));
        let measured = base.with_file_name(format!("{}-measured", base.file_name().unwrap().to_string_lossy()));
        let patches = verify::patches();
        for f in [&expected, &measured] {
            if let Err(e) = verify::write_ti1(&f.with_extension("ti1"), &patches) {
                return fail(self, format!("Can't write the patch set: {e}"));
            }
        }
        // The profile's prediction for each patch (quick, so not a job)
        let out = std::process::Command::new(bin.join("fakeread")).arg(&profile).arg(&expected).output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => return fail(self, format!("fakeread failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
            Err(e) => return fail(self, format!("Can't run fakeread: {e}")),
        }

        let display = &self.inventory.displays[self.display];
        let mut args: Vec<String> = vec!["-v".into(), "-d".into(), display.value.clone()];
        args.extend(self.instrument_args());
        // Measure through the profile's calibration, as the profile assumes
        let cal = profile.with_extension("cal");
        if cal.is_file() {
            args.extend(["-k".into(), cal.display().to_string()]);
        }
        args.push(measured.display().to_string());
        self.start_job(ctx, JobKind::Verify, &bin, "dispread", &args);
        self.verify_run = Some(VerifyRun { profile, base, correction: self.settings.correction.clone() });
        self.verify_outcome = None;
    }

    /// Compare prediction and measurement once dispread has finished.
    fn finish_verification(&mut self, run: VerifyRun) -> Result<VerifyOutcome, String> {
        let bin = self.bin.clone().ok_or("ArgyllCMS wasn't found")?;
        let name = run.base.file_name().unwrap().to_string_lossy().into_owned();
        let expected = run.base.with_file_name(format!("{name}-expected.ti3"));
        let measured = run.base.with_file_name(format!("{name}-measured.ti3"));
        // -N: each reading relative to its own white, so a changed backlight
        // setting isn't counted as error; -k: CIEDE2000
        let out = std::process::Command::new(bin.join("colverify"))
            .args(["-v", "2", "-N", "-k"])
            .arg(&expected)
            .arg(&measured)
            .output()
            .map_err(|e| format!("Can't run colverify: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        let results = verify::parse_colverify(&text);
        let summary = verify::summarize(&results, |i| i <= 21).ok_or("colverify gave no results")?;
        let additivity = std::fs::read_to_string(&measured)
            .ok()
            .and_then(|t| report::read_xyz(&t, verify::patches().len(), false))
            .and_then(|m| verify::additivity(&m));
        let lut = profiles::read(&run.profile).is_some_and(|i| i.lut);
        let kind = profile_type(lut);
        let text = verify::report(&run.profile, kind, &summary, &verify::patches(), &results, run.correction.as_deref(), additivity);
        std::fs::write(run.base.with_extension("txt"), text).map_err(|e| format!("Can't write the report: {e}"))?;
        // The HTML one, with charts, is the one Open shows
        let report = run.base.with_extension("html");
        let curves = profiles::read_curves(&run.profile);
        let page = verify::html(&run.profile, kind, &summary, &verify::patches(), &results, run.correction.as_deref(), additivity, curves.as_ref());
        std::fs::write(&report, page).map_err(|e| format!("Can't write the report: {e}"))?;
        Ok(VerifyOutcome { profile: run.profile, lut, additivity, summary, results, report })
    }

    fn install_profile(&mut self, ctx: &egui::Context) {
        let (Some(bin), Some(outcome)) = (self.bin.clone(), self.outcome.as_ref()) else { return };
        let display = &self.inventory.displays[self.display];
        let args = vec!["-d".into(), display.value.clone(), "-I".into(), outcome.icc.display().to_string()];
        self.start_job(ctx, JobKind::Install, &bin, "dispwin", &args);
    }

    fn start_job(&mut self, ctx: &egui::Context, kind: JobKind, bin: &std::path::Path, tool: &str, args: &[String]) {
        // A profile's later stages add to the first one's log
        if !matches!(kind, JobKind::Targen | JobKind::ProfileRead | JobKind::Colprof) {
            self.log.clear();
        }
        self.log.push(format!("$ {tool} {}", args.join(" ")));
        self.progress = None;
        self.prompt = None;
        self.adjust = adjust::State::default();
        self.adjust_shown = None;
        // Jobs that measure start by asking for the instrument to be placed
        self.placing = matches!(kind, JobKind::Calibrate | JobKind::Verify | JobKind::Report | JobKind::Adjust);
        self.cancelled = false;
        self.status = None;
        let ctx = ctx.clone();
        match Job::start(bin, tool, args, move || ctx.request_repaint()) {
            Ok(job) => self.job = Some((kind, job)),
            Err(e) => self.status = Some((false, format!("Couldn't start {tool}: {e}"))),
        }
    }

    /// Take what the running tool has said since the last frame.
    fn poll_job(&mut self) {
        let Some((kind, job)) = self.job.as_mut() else { return };
        let kind = *kind;
        while let Ok(event) = job.events.try_recv() {
            match event {
                Event::Line(line) => {
                    if kind == JobKind::Adjust {
                        self.adjust.line(&line);
                    }
                    self.log.push(line);
                }
                Event::Live(text) => self.adjust.live(&text),
                Event::Progress(n, m) => self.progress = Some((n, m)),
                Event::Prompt { prompt, before } => self.prompt = Some(prompt_message(&prompt, &before)),
            }
        }
        let Some(code) = job.try_wait() else { return };
        // Pick up anything printed just before exit
        while let Ok(event) = job.events.try_recv() {
            if let Event::Line(line) = event {
                self.log.push(line);
            }
        }
        self.job = None;
        self.prompt = None;
        self.placing = false;
        let cancelled = std::mem::take(&mut self.cancelled);
        let ok = code == Some(0) && !cancelled;
        match kind {
            JobKind::Calibrate | JobKind::Targen | JobKind::ProfileRead | JobKind::Colprof => {
                if cancelled || !ok {
                    if let Some(run) = self.cal_run.take() {
                        run.discard();
                    }
                    self.status = (!cancelled).then(|| {
                        let stage = kind.stage().unwrap_or_default().split(": ").nth(1).unwrap_or_default().to_string();
                        (false, format!("Stopped while {stage}. See the log."))
                    });
                    return;
                }
                if kind == JobKind::Calibrate {
                    // dispcal prints its summary twice (after calibrating
                    // and after verifying); keep the final one
                    let lines: Vec<String> = self
                        .log
                        .iter()
                        .filter(|l| l.contains("error =") || l.starts_with("White point error") || l.contains("neutral error"))
                        .cloned()
                        .collect();
                    if let Some(run) = self.cal_run.as_mut() {
                        run.summary = lines.iter().rev().take(4).rev().cloned().collect();
                    }
                }
                if kind != JobKind::Colprof {
                    let ctx = self.ctx.clone();
                    self.next_stage(&ctx, kind);
                    return;
                }
                let Some(run) = self.cal_run.take() else { return };
                let (cal, icc) = (run.base.with_extension("cal"), run.base.with_extension("icc"));
                if !(cal.is_file() && icc.is_file()) {
                    run.discard();
                    self.status = Some((false, "The profile wasn't written. See the log.".into()));
                    return;
                }
                let _ = std::fs::remove_file(&run.quick);
                let mut summary = run.summary;
                // "Profile check complete, peak err = 2.1, avg err = 0.5"
                summary.extend(self.log.iter().filter(|l| l.contains("peak err")).last().map(|l| l.trim().to_string()));
                self.outcome = Some(Outcome { cal, icc, summary, installed: false });
                self.status = Some((true, "Calibration and profile done".into()));
            }
            JobKind::Verify => {
                let run = self.verify_run.take();
                match (ok, run) {
                    _ if cancelled => self.status = None,
                    (true, Some(run)) => match self.finish_verification(run) {
                        Ok(outcome) => {
                            self.status = Some((true, format!("Verified: {}", outcome.summary.verdict.label())));
                            self.verify_outcome = Some(outcome);
                        }
                        Err(why) => self.status = Some((false, why)),
                    },
                    _ => self.status = Some((false, "Verification didn't finish. See the log.".into())),
                }
            }
            JobKind::InstallRule => {
                self.status = Some(if ok {
                    (true, "Access rule installed. If the instrument still isn't found, unplug it and plug it in again.".into())
                } else {
                    (false, "The access rule wasn't installed. See the log on the Measure page.".into())
                });
                self.refresh_inventory();
            }
            JobKind::Adjust => {
                self.adjust = adjust::State::default();
                self.status = (!ok && !cancelled).then(|| (false, "Adjusting stopped. See the log on the Measure page.".into()));
            }
            JobKind::Report => {
                let run = self.report_run.take();
                match (ok, run) {
                    _ if cancelled => self.status = None,
                    (true, Some(run)) => match self.finish_report(run) {
                        Ok(outcome) => {
                            self.status = None;
                            self.report_outcome = Some(outcome);
                        }
                        Err(why) => self.status = Some((false, why)),
                    },
                    _ => self.status = Some((false, "The report didn't finish. See the log.".into())),
                }
            }
            JobKind::UseProfile | JobKind::StopProfile => {
                self.status = match (ok, kind) {
                    (true, JobKind::UseProfile) => Some((true, "KWin now uses this profile for the display.".into())),
                    (true, _) => Some((true, "The display no longer uses a profile; KWin treats it as sRGB.".into())),
                    (false, _) => Some((false, "Changing the profile failed. See the log.".into())),
                };
                self.refresh_profiles();
            }
            JobKind::Install => {
                if ok {
                    if let Some(o) = self.outcome.as_mut() {
                        o.installed = true;
                        self.verify_profile = Some(o.icc.clone());
                    }
                    self.status = Some((true, "Profile installed. KWin now uses it for this display.".into()));
                } else {
                    self.status = Some((false, "Installing the profile failed. See the log.".into()));
                }
            }
        }
    }

    fn set_theme(&mut self, ctx: &egui::Context, kind: ThemeKind) {
        if kind == self.theme {
            return;
        }
        self.theme = kind;
        theme::apply_kind(ctx, kind);
        self.settings.theme = Some(kind.label().to_string());
        self.save_settings();
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.status = Some((false, format!("Couldn't save preferences: {e}")));
        }
    }

    fn page_preferences(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut prefs = self.prefs.take().unwrap_or_else(|| Prefs::from_settings(&self.settings));
        let t = Tokens::get(&ctx);
        let running = self.job.is_some();

        // Set when the ArgyllCMS choice changes; it takes effect at once if
        // it passes its check
        let mut changed = false;

        // Folder picker result from its thread
        if let Some(rx) = &prefs.picker {
            if let Ok(picked) = rx.try_recv() {
                prefs.picker = None;
                if let Some(dir) = picked {
                    prefs.source = Source::Other;
                    prefs.path = dir.display().to_string();
                    prefs.recheck();
                    changed = true;
                }
            }
        }

        let mut theme = self.theme;
        ui.heading("Preferences");
        Self::gap(ui);
        ui.label("Which ArgyllCMS to use, and how DisplayCraft looks. Changes take effect at once.");
        Self::gap(ui);
        {
                ui.label(RichText::new("ArgyllCMS programs").font(t.semibold(13.0)).color(t.text));
                ui.add_space(4.0);
                if let Some(session) = prefs.session.as_ref().filter(|s| s.automatic.is_some()) {
                    ui.label(RichText::new(format!("This session: {}", session.describe())).color(t.text_dim));
                    ui.add_space(4.0);
                }
                if running {
                    ui.label(RichText::new("Can't change while a calibration runs").color(t.text_dim));
                }
                ui.add_enabled_ui(!running, |ui| {
                    let bundled = system::current().bundled_dir();
                    // What auto amounts to, once the bundled ArgyllCMS has said
                    let detected = prefs.session.as_ref().filter(|_| prefs.source == Source::Bundled(None)).and_then(|s| s.automatic);
                    for variant in system::current().variants() {
                        let (source, label) = (Source::Bundled(variant.window_system), variant.label);
                        ui.horizontal(|ui| {
                            let radio = ui.add_enabled(bundled.is_some(), egui::RadioButton::new(prefs.source == source, label));
                            if radio.clicked() {
                                prefs.source = source;
                                prefs.recheck();
                                changed = true;
                            }
                            if let Some(dir) = &bundled {
                                radio.on_hover_text(dir.display().to_string());
                            }
                            if let (Source::Bundled(None), Some(ws)) = (source, detected) {
                                ui.label(RichText::new(format!("uses {}", ws.label())).color(t.text_dim));
                            }
                        });
                    }
                    if ui.radio(prefs.source == Source::Other, "Other ArgyllCMS, in this directory:").clicked() {
                        prefs.source = Source::Other;
                        prefs.recheck();
                        changed = true;
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        let other = prefs.source == Source::Other;
                        let edit = ui.add_enabled(
                            other,
                            egui::TextEdit::singleline(&mut prefs.path).desired_width(300.0),
                        );
                        if edit.lost_focus() && prefs.other_dir() != self.settings.argyll_bin {
                            prefs.recheck();
                            changed = true;
                        }
                        let browsing = prefs.picker.is_some();
                        if ui.add_enabled(other && !browsing, egui::Button::new("Browse…")).clicked() {
                            let (tx, rx) = std::sync::mpsc::channel();
                            let start = prefs.other_dir().filter(|p| p.is_dir());
                            let ctx = ctx.clone();
                            std::thread::spawn(move || {
                                let mut dialog = rfd::FileDialog::new().set_title("ArgyllCMS programs directory");
                                if let Some(dir) = start {
                                    dialog = dialog.set_directory(dir);
                                }
                                let _ = tx.send(dialog.pick_folder());
                                ctx.request_repaint();
                            });
                            prefs.picker = Some(rx);
                        }
                    });
                });
                ui.add_space(4.0);
                match &prefs.check {
                    Some(Ok(version)) => ui.label(RichText::new(format!("Found ArgyllCMS {version}")).color(t.good)),
                    Some(Err(why)) => {
                        let still = match &self.bin {
                            Some(bin) => format!(". Still using {}.", bin.display()),
                            None => String::new(),
                        };
                        ui.label(RichText::new(format!("{why}{still}")).color(t.bad))
                    }
                    None => ui.label(""),
                };
                if prefs.check.as_ref().is_some_and(|c| c.is_ok()) {
                    match prefs.session.as_ref().map(|s| &s.platform) {
                        Some(Platform::Supported(what)) => ui.label(RichText::new(format!("Supported: {what}")).color(t.good)),
                        Some(Platform::Limited(what)) => ui.label(RichText::new(format!("Limited: {what}")).color(t.caution)),
                        Some(Platform::Unsupported(why)) => ui.label(RichText::new(format!("Not supported: {why}")).color(t.bad)),
                        Some(Platform::Unknown) => ui.label(
                            RichText::new("This ArgyllCMS can't say whether it supports this session").color(t.caution),
                        ),
                        None => ui.label(""),
                    };
                }

                Self::gap(ui);
                ui.label(RichText::new("Appearance").font(t.semibold(13.0)).color(t.text));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    egui::ComboBox::from_id_salt("theme").selected_text(theme.label()).show_ui(ui, |ui| {
                        for k in ThemeKind::ALL {
                            ui.selectable_value(&mut theme, k, k.label());
                        }
                    });
                });
        }

        // The theme applies at once, so it can be judged by eye
        self.set_theme(&ctx, theme);

        // Every change rescans displays and instruments, even choosing the
        // one already in use, which is a way to look again
        if changed && !running && prefs.check.as_ref().is_some_and(|c| c.is_ok()) {
            self.settings.argyll_bin = if prefs.source == Source::Other { prefs.other_dir() } else { None };
            self.settings.argyll_variant = prefs.variant();
            Self::tell_window_system(&self.settings);
            self.save_settings();
            self.bin = argyll::find_bin_dir(self.settings.argyll_bin.as_deref());
            self.corrections = corrections::find(self.bin.as_deref());
            self.refresh_inventory();
        }
        self.prefs = Some(prefs);
    }

    fn page_about(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        // A link that opens with the desktop's browser: egui's own
        // hyperlinks need a browser-opening feature eframe isn't built with
        let link = |ui: &mut egui::Ui, text: &str, url: &str| {
            if ui.link(text).on_hover_text(url).clicked() {
                system::current().open(OsStr::new(url));
            }
        };
        let section = |ui: &mut egui::Ui, title: &str| {
            Self::gap(ui);
            ui.label(RichText::new(title).font(t.semibold(13.0)).color(t.text));
            ui.add_space(4.0);
        };

        ui.horizontal(|ui| {
            ui.add(logo(72.0));
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(concat!("DisplayCraft ", env!("CARGO_PKG_VERSION"))).font(t.semibold(18.0)).color(t.text));
                ui.label(RichText::new("Display calibration and profiling").color(t.text_dim));
            });
        });

        section(ui, "DisplayCraft");
        ui.label("Copyright © 2026 Jan Brittenson. Licensed under the MIT License or the Apache License 2.0, at your option.");
        ui.horizontal_wrapped(|ui| {
            ui.label("Inspired by");
            link(ui, "DisplayCAL", "https://displaycal.net");
            ui.label("by Florian Höch.");
        });
        ui.horizontal(|ui| {
            link(ui, "Source and releases on GitHub", "https://github.com/bson/displaycraft");
            ui.label(RichText::new("·").color(t.text_dim));
            link(ui, "MIT License", "https://github.com/bson/displaycraft/blob/main/LICENSE-MIT");
            ui.label(RichText::new("·").color(t.text_dim));
            link(ui, "Apache License 2.0", "https://github.com/bson/displaycraft/blob/main/LICENSE-APACHE");
        });

        section(ui, "ArgyllCMS");
        let version = if self.inventory.version.is_empty() { String::new() } else { format!(" {}", self.inventory.version) };
        ui.label(format!(
            "Measuring, calibrating and profiling are done by ArgyllCMS{version}, by Graeme W. Gill, which DisplayCraft \
             runs as separate programs. It's licensed under the GNU Affero General Public License version 3."
        ));
        ui.add_space(4.0);
        ui.label(
            "The ArgyllCMS that comes with DisplayCraft is modified to run natively on KDE Plasma under Wayland, as \
             well as on X11, and to reconnect an instrument that stops responding. Its complete source:",
        );
        ui.horizontal(|ui| {
            link(ui, "Modified ArgyllCMS on GitHub", "https://github.com/bson/ArgyllCMS");
            ui.label(RichText::new("·").color(t.text_dim));
            link(ui, "GNU AGPL 3.0", "https://github.com/bson/ArgyllCMS/blob/main/License.txt");
            ui.label(RichText::new("·").color(t.text_dim));
            link(ui, "ArgyllCMS home page", "https://www.argyllcms.com");
        });

        section(ui, "Also included");
        ui.horizontal_wrapped(|ui| {
            ui.label("The look is adapted from the theme of");
            link(ui, "LightCraft", "https://github.com/storytold/lightcraft");
            ui.label("(MIT or Apache 2.0), and text is set in");
            link(ui, "Inter", "https://rsms.me/inter/");
            ui.label("(SIL Open Font License 1.1). DisplayCraft is not affiliated with or endorsed by ArtCraft, but is built around its libraries and \
                      other technologies.");
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.add_space(12.0);
        ui.vertical_centered(|ui| ui.add(logo(112.0)));
        ui.add_space(10.0);
        let numbered = Step::WORKFLOW.into_iter().enumerate().map(|(i, s)| (Some(i + 1), s));
        let tools = Step::TOOLS.into_iter().map(|s| (None, s));
        for (number, step) in numbered.chain(tools) {
            if number.is_none() && step == Step::TOOLS[0] {
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
            }
            let selected = self.step == step;
            let label = match number {
                Some(n) => format!("{n}  {}", step.label()),
                None => step.label().to_string(),
            };
            let text = RichText::new(label).color(if selected { t.text } else { t.text_label });
            let button = egui::Button::new(text).selected(selected).min_size(egui::vec2(ui.available_width(), 30.0));
            // Settings can't change under a running job
            let job_step = self.job.as_ref().map(|(kind, _)| kind.step());
            let usable = self.unsupported().is_none();
            if ui.add_enabled(usable && (job_step.is_none() || Some(step) == job_step), button).clicked() && self.step != step {
                self.step = step;
                if step == Step::Profiles {
                    self.refresh_profiles();
                }
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(8.0);
            if !self.inventory.version.is_empty() {
                ui.label(RichText::new(format!("ArgyllCMS {}", self.inventory.version)).small().color(t.text_dim));
            }
            ui.add_space(8.0);
            // Bottom up: About ends up below Preferences
            for step in [Step::About, Step::Preferences] {
                let selected = self.step == step;
                let text = RichText::new(step.label()).color(if selected { t.text } else { t.text_label });
                let button = egui::Button::new(text).selected(selected).min_size(egui::vec2(ui.available_width(), 30.0));
                if ui.add(button).clicked() {
                    self.step = step;
                }
            }
        });
    }

    /// Shown instead of every step when ArgyllCMS can't run here.
    fn page_unsupported(&mut self, ui: &mut egui::Ui, why: &str) {
        let t = Tokens::get(ui.ctx());
        ui.heading("DisplayCraft can't run here");
        ui.add_space(6.0);
        ui.label("ArgyllCMS can't show test patches or load calibrations in this session:");
        ui.label(RichText::new(why).color(t.bad));
        if let Some(session) = self.session.as_ref().filter(|s| s.automatic.is_some()) {
            ui.label(RichText::new(format!("This session: {}", session.describe())).color(t.text_dim));
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "DisplayCraft needs KDE Plasma on Wayland, or an X11 session (GNOME, KDE or others), with the \
                 ArgyllCMS it comes with. In Preferences, the bundled ArgyllCMS can be set to the other of the two.",
            )
            .color(t.text_dim),
        );
        if let Some(bin) = &self.bin {
            ui.label(RichText::new(format!("Checked with {}", bin.join("dispwin").display())).small().color(t.text_dim));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Check again").clicked() {
                self.refresh_inventory();
            }
            if ui.button("Preferences").clicked() {
                self.step = Step::Preferences;
            }
        });
    }

    fn page_display(&mut self, ui: &mut egui::Ui) {
        ui.heading("Display");
        Self::gap(ui);
        ui.label(
            "Select a display to calibrate. During the calibration process, colored patches will be displayed at \
             its center, so if this window is at the center you may wish to move it aside.",
        );
        Self::gap(ui);
        if self.scan.is_some() {
            Self::looking(ui);
        } else if self.inventory.displays.is_empty() {
            ui.label("No displays found.");
        }
        if !self.layout.is_empty() && !self.inventory.displays.is_empty() {
            if let Some(i) = self.display_map(ui) {
                if i != self.display && self.job.is_none() {
                    self.display = i;
                    self.profiles_view = None;
                }
            }
            Self::gap(ui);
        }
        if let Some(current) = self.inventory.displays.get(self.display) {
            let mut chosen = self.display;
            let running = self.job.is_some();
            ui.add_enabled_ui(!running, |ui| {
                egui::ComboBox::from_id_salt("display").width(360.0).selected_text(display_name(&current.description)).show_ui(ui, |ui| {
                    for (i, d) in self.inventory.displays.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, display_name(&d.description)).on_hover_text(&d.description);
                    }
                });
            });
            // Where it is and how big, from the rest of Argyll's description
            if let Some((_, place)) = current.description.split_once(" at ") {
                Self::context_line(ui, format!("At {place}"));
            }
            if chosen != self.display {
                self.display = chosen;
                self.profiles_view = None;
            }
        }
        Self::gap(ui);
        if ui.add_enabled(self.scan.is_none(), egui::Button::new("Refresh")).clicked() {
            self.refresh_inventory();
        }
    }

    /// The displays as they're arranged; clicking one returns its index in
    /// Argyll's display list. Outputs Argyll doesn't list, or that are off
    /// or mirroring another, are drawn dimmed and can't be chosen.
    fn display_map(&self, ui: &mut egui::Ui) -> Option<usize> {
        let t = Tokens::get(ui.ctx());
        let shown: Vec<&layout::Output> = self.layout.iter().filter(|o| o.enabled && !o.mirrored).collect();
        if shown.is_empty() {
            return None;
        }
        let (x0, y0) = shown.iter().fold((f64::MAX, f64::MAX), |(x, y), o| (x.min(o.x), y.min(o.y)));
        let (x1, y1) = shown.iter().fold((f64::MIN, f64::MIN), |(x, y), o| (x.max(o.x + o.width), y.max(o.y + o.height)));
        let width = ui.available_width().min(600.0);
        let height = 190.0_f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let scale = ((width - 8.0) as f64 / (x1 - x0)).min((height - 8.0) as f64 / (y1 - y0));
        // Centered in the area
        let (dw, dh) = (((x1 - x0) * scale) as f32, ((y1 - y0) * scale) as f32);
        let origin = rect.min + egui::vec2((width - dw) / 2.0, (height - dh) / 2.0);
        let painter = ui.painter_at(rect);
        let mut clicked = None;
        for o in shown {
            let r = egui::Rect::from_min_size(
                origin + egui::vec2(((o.x - x0) * scale) as f32, ((o.y - y0) * scale) as f32),
                egui::vec2((o.width * scale) as f32, (o.height * scale) as f32),
            )
            .shrink(3.0);
            let index = o.argyll_index.or_else(|| self.inventory.displays.iter().position(|d| output_name(&d.description) == o.name));
            let response = ui.interact(r, ui.id().with(("display-map", &o.name)), if index.is_some() { egui::Sense::click() } else { egui::Sense::hover() });
            // Named for screen readers (and tests), like a button
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, index.is_some(), &o.name));
            let selected = index == Some(self.display);
            let fill = if selected {
                t.accent.gamma_multiply(0.55)
            } else if response.hovered() && index.is_some() {
                t.hover
            } else {
                t.field
            };
            painter.rect(r, 4, fill, egui::Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { t.button_border }), egui::StrokeKind::Inside);
            let model = index
                .and_then(|i| self.inventory.displays.get(i))
                .map(|d| display_name(&d.description))
                .and_then(|n| n.split_once(" (").map(|(_, m)| m.trim_end_matches(')').to_string()))
                .unwrap_or_default();
            let mut lines = vec![(o.name.clone(), t.semibold(13.0), t.text)];
            if !model.is_empty() && model != o.name {
                lines.push((model, egui::FontId::proportional(11.0), t.text_label));
            }
            lines.push((format!("{} × {}", o.pixels.0, o.pixels.1), egui::FontId::proportional(11.0), t.text_dim));
            if o.primary {
                lines.push(("primary".into(), egui::FontId::proportional(10.5), t.text_dim));
            }
            if index.is_none() {
                lines.push(("not available to ArgyllCMS".into(), egui::FontId::proportional(10.5), t.caution));
            }
            let total: f32 = lines.iter().map(|(_, f, _)| f.size + 3.0).sum();
            let mut y = r.center().y - total / 2.0;
            for (text, font, color) in lines {
                let size = font.size;
                // Clipped to the rectangle, so a small output's labels don't spill
                ui.painter_at(r).text(egui::pos2(r.center().x, y), egui::Align2::CENTER_TOP, text, font, color);
                y += size + 3.0;
            }
            if response.clicked() {
                clicked = index;
            }
            response.on_hover_text(match index {
                Some(_) => format!("{} at {}, {}", o.name, o.x, o.y),
                None => format!("{} isn't in ArgyllCMS's display list", o.name),
            });
        }
        // Outputs that are connected but not shown
        let hidden: Vec<String> = self
            .layout
            .iter()
            .filter(|o| !o.enabled || o.mirrored)
            .map(|o| format!("{} ({})", o.name, if o.mirrored { "mirroring" } else { "off" }))
            .collect();
        if !hidden.is_empty() {
            Self::context_line(ui, format!("Also connected: {}", hidden.join(", ")));
        }
        clicked
    }

    fn page_instrument(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Spectrophotometer or Colorimeter");
        Self::gap(ui);
        ui.label("Select the measurement device to use.");
        Self::gap(ui);
        if self.scan.is_some() {
            Self::looking(ui);
        } else if self.inventory.instruments.is_empty() {
            ui.label(RichText::new("No instrument found. Connect one, then refresh.").color(t.caution));
        }
        for (i, inst) in self.inventory.instruments.iter().enumerate() {
            ui.radio_value(&mut self.instrument, i, instrument_name(&inst.description)).on_hover_text(&inst.description);
        }
        if !self.blocked.is_empty() && self.scan.is_none() {
            Self::gap(ui);
            self.access_notice(ui);
        }
        Self::gap(ui);
        if ui.add_enabled(self.scan.is_none(), egui::Button::new("Refresh")).clicked() {
            self.refresh_inventory();
        }
        self.corrections_section(ui);
    }

    /// Instruments that are plugged in but can't be opened, and the offer to
    /// install the access rule
    fn access_notice(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        let names: Vec<&str> = self.blocked.iter().map(|b| b.name.as_str()).collect();
        let running = self.job.is_some();
        let mut install = false;
        egui::Frame::new().fill(t.inset).corner_radius(4).inner_margin(10).show(ui, |ui| {
            ui.label(RichText::new(format!("No permission to use: {}", names.join(", "))).color(t.caution));
            ui.add_space(4.0);
            ui.label(
                "The instrument is plugged in, but this system doesn't give you access to it. ArgyllCMS's device \
                 rule does, for every instrument it supports; installing it needs an administrator password.",
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!running, egui::Button::new("Install access rule…")).clicked() {
                    install = true;
                }
                ui.label(RichText::new(format!("Installs {} into /etc/udev/rules.d", usbaccess::RULES_NAME)).small().color(t.text_dim));
            });
        });
        if install {
            self.install_access_rule(ui.ctx());
        }
    }

    fn install_access_rule(&mut self, ctx: &egui::Context) {
        let path = match Self::data_dir().map(|d| d.join("work")) {
            Ok(dir) if std::fs::create_dir_all(&dir).is_ok() => dir.join(usbaccess::RULES_NAME),
            _ => {
                self.status = Some((false, "Can't create the work directory".into()));
                return;
            }
        };
        // pkexec's root shell copies the rule from where the user wrote it
        if let Err(e) = std::fs::write(&path, usbaccess::RULES) {
            self.status = Some((false, format!("Can't write {}: {e}", path.display())));
            return;
        }
        let args = vec!["sh".to_string(), "-c".to_string(), usbaccess::install_command(&path)];
        let ctx = ctx.clone();
        self.start_job(&ctx, JobKind::InstallRule, std::path::Path::new("/usr/bin"), "pkexec", &args);
    }

    fn corrections_section(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        if let Some(rx) = &self.correction_picker {
            if let Ok(picked) = rx.try_recv() {
                self.correction_picker = None;
                if let Some(path) = picked {
                    match corrections::read(&path) {
                        Some(c) => {
                            if !self.corrections.iter().any(|k| k.path == c.path) {
                                self.corrections.insert(0, c);
                            }
                            self.set_correction(Some(path));
                        }
                        None => self.status = Some((false, format!("{} isn't a .ccss or .ccmx file", path.display()))),
                    }
                }
            }
        }
        Self::gap(ui);
        ui.label(RichText::new("Correction").font(t.semibold(13.0)).color(t.text));
        Self::gap(ui);
        ui.label("An optional spectral correction (.ccss) or matrix (.ccmx) can be selected for the backlight.");
        Self::gap(ui);
        // One choice covers both kinds: a display type selects one of the
        // instrument's built-in corrections (-y), a file is passed with -X
        #[derive(Clone, PartialEq)]
        enum Pick {
            Default,
            Type(usize),
            File(PathBuf),
        }
        let current = match (&self.settings.correction, self.display_type) {
            (Some(path), _) => Pick::File(path.clone()),
            (None, Some(i)) => Pick::Type(i),
            (None, None) => Pick::Default,
        };
        let mut chosen = current.clone();
        const NONE: &str = "None (use Instrument Default)";
        let selected = match &chosen {
            Pick::Default => NONE.to_string(),
            Pick::Type(i) => self.inventory.display_types.get(*i).map(|dt| dt.description.clone()).unwrap_or_default(),
            Pick::File(path) => match self.corrections.iter().find(|c| &c.path == path) {
                Some(c) => c.label(),
                // A saved choice that's no longer in the usual places
                None => path.display().to_string(),
            },
        };
        let browsing = self.correction_picker.is_some();
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 90.0).clamp(200.0, 560.0);
            egui::ComboBox::from_id_salt("correction").width(width).selected_text(selected).show_ui(ui, |ui| {
                // Some descriptions are long enough to push the list off
                // the window; they're cut short and shown whole on hover
                ui.set_max_width(width);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                ui.selectable_value(&mut chosen, Pick::Default, NONE);
                if !self.inventory.display_types.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Built into the instrument").small().color(t.text_dim));
                    for (i, dt) in self.inventory.display_types.iter().enumerate() {
                        ui.selectable_value(&mut chosen, Pick::Type(i), &dt.description).on_hover_text(&dt.description);
                    }
                }
                if !self.corrections.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("Correction files").small().color(t.text_dim));
                    for c in &self.corrections {
                        ui.selectable_value(&mut chosen, Pick::File(c.path.clone()), c.label())
                            .on_hover_text(format!("{}\n{}", c.label(), c.path.display()));
                    }
                }
            });
            if ui.add_enabled(!browsing, egui::Button::new("Browse…")).clicked() {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let picked = rfd::FileDialog::new()
                        .set_title("Colorimeter correction")
                        .add_filter("Corrections", &["ccss", "ccmx"])
                        .pick_file();
                    let _ = tx.send(picked);
                    ctx.request_repaint();
                });
                self.correction_picker = Some(rx);
            }
        });
        if chosen != current {
            match chosen {
                Pick::Default => {
                    self.display_type = None;
                    self.set_correction(None);
                }
                Pick::Type(i) => {
                    self.display_type = Some(i);
                    self.set_correction(None);
                }
                Pick::File(path) => {
                    self.display_type = None;
                    self.set_correction(Some(path));
                }
            }
        }
    }

    fn set_correction(&mut self, path: Option<PathBuf>) {
        self.settings.correction = path;
        self.save_settings();
    }

    fn page_targets(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Settings");
        Self::gap(ui);
        if let Some(d) = self.display_label() {
            let backlight = self.correction_label().unwrap_or_else(|| "instrument default".into());
            Self::context_line(ui, format!("Display: {d}  ·  Backlight correction: {backlight}"));
        }
        let before = self.settings.targets.clone();
        let running = self.job.is_some();
        let ready = self.ready().is_ok();
        let mut adjust_clicked = false;
        Self::gap(ui);
        ui.horizontal(|ui| {
            ui.label("Display controls");
            ui.add_space(8.0);
            if ui
                .add_enabled(!running && ready, egui::Button::new("Adjust…"))
                .on_hover_text(
                    "Set the display's own white point, brightness and black level from its menus while \
                     DisplayCraft shows how close they are to the targets below. Worth it for monitors with those \
                     controls; laptop panels usually have only brightness.",
                )
                .clicked()
            {
                adjust_clicked = true;
            }
        });
        // An adjustment runs right here, near the top, so its buttons are in
        // view: further down they could be scrolled off, where clicks don't
        // reach them
        if self.job.as_ref().is_some_and(|(k, _)| *k == JobKind::Adjust) {
            // The placement prompt, or nothing once dispcal is past it
            if self.placing || self.prompt.is_some() || !self.adjust.active() {
                Self::gap(ui);
                let controls = ui.scope(|ui| {
                    self.job_controls(ui, JobKind::Adjust, false, true);
                });
                if self.prompt.is_some() && self.adjust_shown.is_none() {
                    controls.response.scroll_to_me(Some(Align::Max));
                }
            }
            if self.adjust.active() {
                Self::gap(ui);
                self.adjust_panel(ui);
            }
            Self::gap(ui);
            self.cancel_button(ui);
        }
        Self::gap(ui);
        ui.label("What the calibration should aim for.");
        Self::gap(ui);
        let tg = &mut self.settings.targets;
        // Fixed while adjusting: dispcal took the targets when it started
        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("targets").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label("White point");
                choices(ui, |ui| {
                    for (w, name, k) in White::PRESETS {
                        ui.radio_value(&mut tg.white, w, name).on_hover_text(format!("Daylight, {k:.0} K"));
                    }
                    ui.radio_value(&mut tg.white, White::Native, "Native")
                        .on_hover_text("The display's own white, with no color temperature correction");
                    with_field(ui, |ui| {
                        ui.radio_value(&mut tg.white, White::Custom, "Custom");
                        ui.add_enabled(tg.white == White::Custom, egui::DragValue::new(&mut tg.kelvin).range(3000.0..=10000.0).speed(10.0).suffix(" K"));
                    });
                });
                ui.end_row();

                ui.label("Tonal response");
                choices(ui, |ui| {
                    ui.radio_value(&mut tg.tone, Tone::Gamma22, "Gamma 2.2");
                    ui.radio_value(&mut tg.tone, Tone::Gamma18, "Gamma 1.8");
                    ui.radio_value(&mut tg.tone, Tone::Srgb, "sRGB curve");
                    with_field(ui, |ui| {
                        ui.radio_value(&mut tg.tone, Tone::Gamma, "Custom");
                        ui.add_enabled(tg.tone == Tone::Gamma, egui::DragValue::new(&mut tg.gamma).range(1.0..=3.0).speed(0.01).fixed_decimals(2));
                    });
                });
                ui.end_row();

                ui.label("White brightness");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut tg.set_brightness, "Set to");
                    ui.add_enabled(tg.set_brightness, egui::DragValue::new(&mut tg.brightness).range(40.0..=600.0).speed(1.0).suffix(" cd/m²"));
                    if !tg.set_brightness {
                        ui.label(RichText::new("(native)").color(t.text_dim));
                    }
                });
                ui.end_row();

                ui.label("Quality");
                choices(ui, |ui| {
                    let low = Targets::allows(Quality::Low, tg.profile);
                    ui.add_enabled_ui(low, |ui| ui.radio_value(&mut tg.quality, Quality::Low, "Low"))
                        .response
                        .on_disabled_hover_text("Too few patches for a LUT profile");
                    ui.radio_value(&mut tg.quality, Quality::Medium, "Medium");
                    ui.radio_value(&mut tg.quality, Quality::High, "High");
                });
                ui.end_row();

                ui.label("Profile type");
                choices(ui, |ui| {
                    ui.radio_value(&mut tg.profile, ProfileType::ShaperMatrix, "Shaper+matrix")
                        .on_hover_text("Smooth, compact, and understood by every application");
                    let lut = Targets::allows(tg.quality, ProfileType::Lut);
                    with_field(ui, |ui| {
                        ui.add_enabled_ui(lut, |ui| {
                            ui.radio_value(&mut tg.profile, ProfileType::Lut, "LUT")
                                .on_hover_text("A table of measured colors, rather than curves and a matrix")
                        })
                        .response
                        .on_disabled_hover_text("Needs Medium or High quality: Low measures too few patches for a LUT");
                        help(
                            ui,
                            "A LUT profile helps most with displays of poor linearity: where the red, green and blue \
                             channels don't add up exactly, or the white point shifts with brightness, as on many OLEDs. \
                             On a display that behaves linearly, like most LCDs, Shaper+matrix is as accurate or more so.\n\n\
                             Try Shaper+matrix first. If Verify finds the result lacking and the display's channels don't \
                             add up, it suggests making a LUT profile.",
                        );
                    });
                });
                ui.end_row();
            });
        });
        Self::gap(ui);
        ui.label(RichText::new(format!(
            "A higher quality setting requires more measurements and will take longer to perform. Low, Medium and High \
             measure {}, {} and {} patches for the profile, at about a second each.",
            Quality::Low.patches(),
            Quality::Medium.patches(),
            Quality::High.patches()
        )).color(t.text_dim));
        if self.settings.targets != before {
            self.save_settings();
        }

        if adjust_clicked {
            let ctx = ui.ctx().clone();
            self.start_adjust(&ctx);
        }

    }

    /// Run dispcal's adjustment of the display's own controls, toward the
    /// white point and brightness targets
    fn start_adjust(&mut self, ctx: &egui::Context) {
        let Some(bin) = self.bin.clone() else { return };
        let Ok(work) = Self::data_dir().map(|d| d.join("work")) else { return };
        if std::fs::create_dir_all(&work).is_err() {
            self.status = Some((false, "Can't create the work directory".into()));
            return;
        }
        let display = &self.inventory.displays[self.display];
        let t = &self.settings.targets;
        let mut args: Vec<String> = vec!["-v".into(), "-d".into(), display.value.clone()];
        args.extend(self.instrument_args());
        if let Some(k) = t.kelvin() {
            args.extend(["-t".into(), format!("{k:.0}")]);
        }
        if t.set_brightness {
            args.extend(["-b".into(), format!("{:.0}", t.brightness)]);
        }
        // Left with Exit, so nothing is written under this name
        args.push(work.join("adjust").display().to_string());
        self.start_job(ctx, JobKind::Adjust, &bin, "dispcal", &args);
    }

    fn page_run(&mut self, ui: &mut egui::Ui) {
        // While a run goes, Cancel stays at the foot of the page, whatever
        // is shown above it
        if self.job.is_some() {
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                self.cancel_button(ui);
                Self::gap(ui);
                ui.with_layout(Layout::top_down(Align::Min), |ui| self.page_run_body(ui));
            });
        } else {
            self.page_run_body(ui);
        }
    }

    fn page_run_body(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Measurement");
        Self::gap(ui);
        if let Some(inst) = self.inventory.instruments.get(self.instrument) {
            let correction = self.correction_label().map(|c| format!(", corrected for {c}")).unwrap_or_default();
            Self::context_line(ui, format!("Instrument: {}{correction}", instrument_name(&inst.description)));
            Self::gap(ui);
        }
        Self::context_line(ui, format!("Goals: {}", self.settings.targets.describe()));
        Self::gap(ui);
        let running = self.job.is_some();
        if self.job_controls(ui, JobKind::Calibrate, false, true) {
            let ctx = ui.ctx().clone();
            self.start_calibration(&ctx);
        }


        if let Some(outcome) = &self.outcome {
            Self::gap(ui);
            egui::Frame::new().fill(t.inset).corner_radius(4).inner_margin(10).show(ui, |ui| {
                ui.label(RichText::new("Result").font(t.semibold(13.0)).color(t.text));
                for line in &outcome.summary {
                    ui.label(line);
                }
                ui.label(RichText::new(format!("Calibration: {}", outcome.cal.display())).small().color(t.text_dim));
                ui.label(RichText::new(format!("Profile: {}", outcome.icc.display())).small().color(t.text_dim));
            });
            let installed = outcome.installed;
            if !running && !installed && ui.button("Install profile").clicked() {
                let ctx = ui.ctx().clone();
                self.install_profile(&ctx);
            }
        }

        Self::gap(ui);
        egui::CollapsingHeader::new("Log").default_open(false).show(ui, |ui| self.log_view(ui, f32::INFINITY));
    }

    fn start_report(&mut self, ctx: &egui::Context) {
        let Some(bin) = self.bin.clone() else { return };
        let fail = |app: &mut App, why: String| app.status = Some((false, why));
        let dir = match Self::data_dir().map(|d| d.join("reports")) {
            Ok(d) => d,
            Err(e) => return fail(self, format!("No place for the report: {e}")),
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return fail(self, format!("Can't create {}: {e}", dir.display()));
        }
        let display = &self.inventory.displays[self.display];
        let output = output_name(&display.description);
        let base = dir.join(format!("{output}-report-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        if let Err(e) = report::write_ti1(&base.with_extension("ti1")) {
            return fail(self, format!("Can't write the patch set: {e}"));
        }
        let mut args: Vec<String> = vec!["-v".into(), "-d".into(), display.value.clone()];
        args.extend(self.instrument_args());
        if self.report_mode == ReportMode::Uncalibrated {
            // dispread otherwise measures through the calibration in effect
            let linear = dir.join("linear.cal");
            if let Err(e) = report::write_linear_cal(&linear) {
                return fail(self, format!("Can't write {}: {e}", linear.display()));
            }
            args.extend(["-k".into(), linear.display().to_string()]);
        }
        args.push(base.display().to_string());
        self.start_job(ctx, JobKind::Report, &bin, "dispread", &args);
        self.report_run = Some(ReportRun { base, mode: self.report_mode, output });
        self.report_outcome = None;
    }

    fn finish_report(&mut self, run: ReportRun) -> Result<ReportOutcome, String> {
        let text = std::fs::read_to_string(run.base.with_extension("ti3")).map_err(|e| format!("Can't read the measurements: {e}"))?;
        let measured = report::read_ti3(&text).ok_or("The measurements are incomplete")?;
        let r = report::analyze(&measured).ok_or("White measured no brighter than black. Was the instrument on the screen?")?;
        let title = format!("{}, {}", run.output, run.mode.title());
        let saved = run.base.with_extension("txt");
        std::fs::write(&saved, report::text(&r, &title)).map_err(|e| format!("Can't write the report: {e}"))?;
        Ok(ReportOutcome { report: r, title, saved })
    }

    fn page_report(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Display Report");
        Self::gap(ui);
        ui.label(
            "Measures what the display does: its brightness, contrast, gamma, white point and how much of the \
             standard color spaces it can show. Measuring it uncalibrated shows what calibration has to work with.",
        );
        Self::gap(ui);
        let running = self.job.is_some();
        ui.add_enabled_ui(!running, |ui| {
            choices(ui, |ui| {
                ui.radio_value(&mut self.report_mode, ReportMode::Uncalibrated, "Uncalibrated");
                ui.radio_value(&mut self.report_mode, ReportMode::Current, "As calibrated now");
            });
        });
        Self::gap(ui);
        if self.job_controls(ui, JobKind::Report, true, true) {
            let ctx = ui.ctx().clone();
            self.start_report(&ctx);
        }

        if let Some(o) = &self.report_outcome {
            let r = &o.report;
            Self::gap(ui);
            egui::Frame::new().fill(t.inset).corner_radius(4).inner_margin(10).show(ui, |ui| {
                ui.label(RichText::new(&o.title).font(t.semibold(13.0)).color(t.text));
                ui.add_space(4.0);
                let value = |ui: &mut egui::Ui, text: String| ui.label(RichText::new(text).color(t.text));
                ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                egui::Grid::new("report").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    ui.label("White luminance");
                    value(ui, format!("{:.1} cd/m²", r.white));
                    ui.end_row();
                    ui.label("Black luminance");
                    value(ui, format!("{:.3} cd/m²", r.black));
                    ui.end_row();
                    ui.label("Contrast ratio");
                    value(ui, format!("{:.0}:1", r.contrast));
                    ui.end_row();
                    ui.label("Gamma");
                    value(ui, format!("{:.2}", r.gamma));
                    ui.end_row();
                    ui.label("White point");
                    value(ui, format!("x {:.4}, y {:.4}  (about {:.0} K)", r.white_xy.0, r.white_xy.1, r.cct));
                    ui.end_row();
                    for c in &r.coverage {
                        ui.label(format!("{} coverage", c.name));
                        value(ui, format!("{:.1}%  (relative size {:.1}%)", c.covered, c.relative));
                        ui.end_row();
                    }
                });
                ui.add_space(4.0);
                ui.label(RichText::new("Coverage is by area in the CIE 1976 u'v' diagram.").small().color(t.text_dim));
                });
                ui.add_space(16.0);
                charts::gamut(r.primaries, Some(r.white_xy)).show(ui, 300.0, 320.0);
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Saved: {}", o.saved.display())).small().color(t.text_dim));
                    if ui.small_button("Open").clicked() {
                        system::current().open(o.saved.as_os_str());
                    }
                });
            });
        }

        Self::gap(ui);
        egui::CollapsingHeader::new("Log").default_open(false).show(ui, |ui| self.log_view(ui, 180.0));
    }

    /// Read the profiles and KWin's state for the Profiles page
    fn refresh_profiles(&mut self) {
        let output = self.inventory.displays.get(self.display).map(|d| output_name(&d.description)).unwrap_or_default();
        let mut rows: Vec<ProfileRow> = Vec::new();
        let verify_dir = Self::data_dir().map(|d| d.join("verify")).ok();
        let mut add = |path: PathBuf, made: bool| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let row = match rows.iter_mut().position(|r| r.name == name) {
                Some(i) => &mut rows[i],
                None => {
                    let info = profiles::read(&path).unwrap_or_default();
                    let verdict = verify_dir.as_deref().and_then(|d| profiles::last_verdict(d, &path));
                    let made_for = made_for(&name).map(str::to_string);
                    let curves = profiles::read_curves(&path);
                    rows.push(ProfileRow { name, info, made: None, installed: None, verdict, made_for, curves });
                    rows.last_mut().unwrap()
                }
            };
            if made { row.made = Some(path) } else { row.installed = Some(path) }
        };
        if let Some(dir) = profiles::installed_dir() {
            for p in profiles::list(&dir) {
                add(p, false);
            }
        }
        if let Ok(dir) = Self::data_dir() {
            for p in profiles::list(&dir) {
                add(p, true);
            }
        }
        // This display's first, then the rest, each newest first
        let other = |r: &ProfileRow| r.made_for.as_deref().is_some_and(|o| o != output);
        rows.sort_by(|a, b| other(a).cmp(&other(b)).then(b.info.created.cmp(&a.info.created)));
        let color = if output.is_empty() { None } else { profiles::output_color(&output) };
        self.profiles_view = Some(ProfilesView { output, color, rows });
    }

    fn page_profiles(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Profiles");
        Self::gap(ui);
        let running = self.job.is_some();
        // The same display as everywhere else; choosing one here changes it
        // for the other pages too
        {
            let mut chosen = self.display;
            ui.horizontal(|ui| {
                ui.label("Display");
                let selected = self.display_label().unwrap_or_else(|| "None found".into());
                ui.add_enabled_ui(!running, |ui| {
                    egui::ComboBox::from_id_salt("profiles-display").width(360.0).selected_text(selected).show_ui(ui, |ui| {
                        for (i, d) in self.inventory.displays.iter().enumerate() {
                            ui.selectable_value(&mut chosen, i, display_name(&d.description));
                        }
                    });
                });
            });
            if chosen != self.display {
                self.display = chosen;
                self.profiles_view = None;
            }
            Self::gap(ui);
        }
        if self.profiles_view.is_none() {
            self.refresh_profiles();
        }
        let manageable = self.scan.is_none() && self.unsupported().is_none() && !self.inventory.displays.is_empty();
        let Some(view) = &self.profiles_view else { return };
        let active = view.color.as_ref().and_then(|c| c.active()).map(PathBuf::from);
        let same = |a: &std::path::Path, b: &std::path::Path| {
            a == b || a.canonicalize().ok().is_some_and(|ca| b.canonicalize().ok().is_some_and(|cb| ca == cb))
        };
        ui.label(format!(
            "The profiles DisplayCraft made, and those installed for displays. KWin uses one for {} at a time.",
            if view.output.is_empty() { "a display".to_string() } else { view.output.clone() }
        ));
        Self::gap(ui);
        match (&view.color, &active) {
            (None, _) => ui.label(RichText::new(format!("KWin didn't say which profile {} uses.", view.output)).color(t.caution)),
            (Some(_), Some(p)) => ui.label(format!("In use: {}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())),
            (Some(_), None) => ui.label("In use: none. KWin treats the display as sRGB."),
        };
        Self::gap(ui);

        let mut action: Option<(JobKind, Option<PathBuf>)> = None;
        let mut delete: Option<PathBuf> = None;
        // A click on a name: Some(None) hides the graphs
        let mut details: Option<Option<String>> = None;
        egui::Grid::new("profiles").num_columns(6).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
            for h in ["Profile", "Created", "White point", "Verified", "", ""] {
                ui.label(RichText::new(h).color(t.text_dim));
            }
            ui.end_row();
            for row in &view.rows {
                let in_use = active.as_deref().is_some_and(|a| row.installed.as_deref().is_some_and(|i| same(a, i)) || row.made.as_deref().is_some_and(|m| same(a, m)));
                // DisplayCraft's profiles go by file name, which is short and
                // dated (their description, on hover, has the targets), as do
                // dispcal's, which it describes by their output path
                let description = &row.info.description;
                let title = if row.made_for.is_some() || description.is_empty() || description.contains('/') {
                    row.name.trim_end_matches(".icc").trim_end_matches(".icm").to_string()
                } else {
                    description.clone()
                };
                let elsewhere = row.made_for.as_deref().filter(|o| *o != view.output);
                let mut where_ = Vec::new();
                if row.made_for.is_some() && !description.is_empty() && !description.contains('/') {
                    where_.push(description.clone());
                }
                if let Some(o) = elsewhere {
                    where_.push(format!("Made for {o}, so it describes that display, not this one."));
                }
                if let Some(p) = &row.made {
                    where_.push(format!("Made by DisplayCraft: {}", p.display()));
                }
                if let Some(p) = &row.installed {
                    where_.push(format!("Installed: {}", p.display()));
                }
                // A fixed width, so a long name (DisplayCAL's run to 60
                // characters) doesn't push the buttons out of the window
                let text = RichText::new(title).color(if in_use { t.text } else { t.text_label });
                ui.allocate_ui_with_layout(egui::vec2(250.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_width(250.0);
                    // Clicking the name shows the profile's graphs
                    let shown = self.profile_details.as_deref() == Some(row.name.as_str());
                    let text = if shown { text.underline() } else { text };
                    let hover = format!("{}\nClick for graphs", where_.join("\n"));
                    if ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click())).on_hover_text(hover).clicked() {
                        details = if shown { Some(None) } else { Some(Some(row.name.clone())) };
                    }
                    if let Some(o) = elsewhere {
                        ui.label(RichText::new(format!("made for {o}")).small().color(t.caution));
                    }
                });
                ui.label(&row.info.created);
                match (row.info.white, row.info.cct()) {
                    (Some((x, y)), Some(k)) => ui.label(format!("~{k:.0} K")).on_hover_text(format!("x {x:.4}, y {y:.4}")),
                    _ => ui.label(RichText::new("—").color(t.text_dim)),
                };
                match row.verdict.as_deref() {
                    Some(v) => {
                        let c = match v {
                            "Good" => t.good,
                            "Acceptable" => t.caution,
                            _ => t.bad,
                        };
                        ui.label(RichText::new(v).color(c))
                    }
                    None => ui.label(RichText::new("—").color(t.text_dim)),
                };
                if in_use {
                    ui.label(RichText::new("In use").color(t.good));
                } else if ui
                    .add_enabled(manageable && !running, egui::Button::new("Use"))
                    .on_hover_text(match elsewhere {
                        Some(o) => format!("Measured on {o}; on this display its colors will be off"),
                        None => "Make KWin use this profile for the display".into(),
                    })
                    .clicked()
                {
                    action = Some((JobKind::UseProfile, Some(row.path().to_path_buf())));
                }
                match &row.made {
                    Some(p) if !in_use => {
                        if ui.add_enabled(!running, egui::Button::new("Delete…")).clicked() {
                            delete = Some(p.clone());
                        }
                    }
                    _ => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
        if view.rows.is_empty() {
            ui.label(RichText::new("No profiles yet.").color(t.text_dim));
        }
        if let Some(row) = self.profile_details.as_ref().and_then(|n| view.rows.iter().find(|r| &r.name == n)) {
            Self::gap(ui);
            profile_charts(ui, &row.name, row.curves.as_ref());
        } else if !view.rows.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Click a profile's name for its graphs.").small().color(t.text_dim));
        }

        Self::gap(ui);
        ui.horizontal(|ui| {
            if active.is_some() && ui.add_enabled(manageable && !running, egui::Button::new("Stop using a profile")).clicked() {
                action = Some((JobKind::StopProfile, None));
            }
            if ui.add_enabled(!running, egui::Button::new("Refresh")).clicked() {
                self.refresh_profiles();
            }
            if let Some((ok, text)) = &self.status {
                ui.label(RichText::new(text).color(if *ok { t.good } else { t.bad }));
            }
        });
        if active.is_some() {
            ui.label(
                RichText::new("Stopping removes the installed copy; a profile DisplayCraft made stays here and can be used again.")
                    .small()
                    .color(t.text_dim),
            );
        }
        if running {
            ui.add_space(6.0);
            self.progress_bar(ui);
        }

        if let Some(p) = delete {
            self.delete_confirm = Some(p);
        }
        if let Some(d) = details {
            self.profile_details = d;
        }
        if let (Some((kind, path)), Some(bin)) = (action, self.bin.clone()) {
            let display = self.inventory.displays[self.display].value.clone();
            let mut args = vec!["-d".to_string(), display];
            match path {
                Some(p) => args.extend(["-I".into(), p.display().to_string()]),
                None => args.push("-U".into()),
            }
            let ctx = ui.ctx().clone();
            self.start_job(&ctx, kind, &bin, "dispwin", &args);
        }
        self.delete_dialog(ui.ctx());
    }

    /// Confirm deleting one of DisplayCraft's profiles, with its calibration
    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = self.delete_confirm.clone() else { return };
        let t = Tokens::get(ctx);
        let mut answer = None;
        egui::Modal::new(egui::Id::new("delete-profile")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.label(RichText::new("Delete this profile?").font(t.semibold(14.0)).color(t.text));
            ui.add_space(6.0);
            ui.label(path.display().to_string());
            ui.label(RichText::new("Its calibration file goes too. Installed copies aren't affected.").color(t.text_dim));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    answer = Some(true);
                }
                if ui.button("Keep").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(true) => {
                let _ = std::fs::remove_file(path.with_extension("cal"));
                self.status = match std::fs::remove_file(&path) {
                    Ok(()) => None,
                    Err(e) => Some((false, format!("Can't delete {}: {e}", path.display()))),
                };
                if self.verify_profile.as_deref() == Some(path.as_path()) {
                    self.verify_profile = self.saved_profiles().into_iter().next();
                }
                self.delete_confirm = None;
                self.refresh_profiles();
            }
            Some(false) => self.delete_confirm = None,
            None => {}
        }
    }

    /// dispcal's adjustment menu, or the live readings of one adjustment
    fn adjust_panel(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        let mut key = None;
        let panel = egui::Frame::new().fill(t.inset).corner_radius(4).inner_margin(12).show(ui, |ui| {
            ui.set_min_width(ui.available_width().min(560.0));
            match self.adjust.mode {
                None => {
                    ui.label(RichText::new("Adjust the display's controls").font(t.semibold(14.0)).color(t.text));
                    ui.add_space(4.0);
                    ui.label(
                        "Use the display's own menus to bring it close to the targets above, so calibration has less \
                         to correct. Choose what to adjust; readings update about once a second.",
                    );
                    Self::gap(ui);
                    ui.horizontal_wrapped(|ui| {
                        for mode in adjust::Mode::ALL {
                            if ui.button(mode.label()).on_hover_text(format!("Turn {}", mode.controls())).clicked() {
                                key = Some(mode.key());
                            }
                        }
                    });
                    Self::gap(ui);
                    if ui.button(RichText::new("Finish").font(t.semibold(13.0))).on_hover_text("Leave the display as adjusted").clicked() {
                        key = Some('8');
                    }
                }
                Some(mode) => {
                    ui.label(RichText::new(mode.label()).font(t.semibold(14.0)).color(t.text));
                    ui.add_space(4.0);
                    ui.label(format!("Turn {} until the arrows settle on =, then press Done.", mode.controls()));
                    if let Some(target) = &self.adjust.target {
                        ui.add_space(4.0);
                        Self::context_line(ui, adjust::target_text(target));
                    }
                    Self::gap(ui);
                    match &self.adjust.reading {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Measuring…");
                            });
                        }
                        Some(r) => reading_view(ui, r),
                    }
                    Self::gap(ui);
                    if ui.button(RichText::new("Done").font(t.semibold(13.0))).clicked() {
                        key = Some(' ');
                    }
                }
            }
        });
        // Bring the whole panel, Done included, into view when it changes
        // between the menu and an adjustment
        let shown = Some(self.adjust.mode);
        if self.adjust_shown != shown {
            self.adjust_shown = shown;
            panel.response.scroll_to_me(Some(Align::Max));
        }
        if let (Some(k), Some((_, job))) = (key, self.job.as_mut()) {
            job.send_key(k);
            // Shown as busy until dispcal's next line says what it's doing
            self.adjust.menu = false;
            if k == '8' {
                self.adjust = adjust::State::default();
            }
        }
    }

    fn cancel_button(&mut self, ui: &mut egui::Ui) {
        if ui.add_enabled(!self.cancelled, egui::Button::new("Cancel")).clicked() {
            if let Some((_, job)) = self.job.as_mut() {
                job.cancel();
            }
            self.cancelled = true;
        }
    }

    /// Begin, and while a job runs, what it's waiting for and Cancel.
    /// Returns true when Begin is clicked. Argyll's first prompt asks for
    /// the instrument to be placed, so a placement message shows from Begin
    /// on, before the prompt itself arrives; Measure answers it. Prompts
    /// with text of their own (e.g. to retry after a failed reading) show
    /// it.
    /// `cancel_inline`: Cancel goes next to Measure, rather than where the
    /// page puts it with `cancel_button`. `status_inline`: the status goes
    /// next to the buttons, rather than where the page shows it.
    fn job_controls(&mut self, ui: &mut egui::Ui, kind: JobKind, cancel_inline: bool, status_inline: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut start = false;
        let running = self.job.as_ref().map(|(k, _)| *k);
        let message = match self.prompt.as_deref() {
            Some(p) => Some(p.to_string()),
            None if self.placing => Some("Open the measurement device and place it on the box at the center of the screen.".to_string()),
            None => None,
        };
        if running.is_some() {
            if let Some(m) = &message {
                ui.label(RichText::new(m).color(t.text));
                ui.add_space(4.0);
            }
        }
        ui.horizontal(|ui| {
            match (self.ready(), running) {
                (Ok(()), None) => {
                    if ui.button(RichText::new("Begin").font(t.semibold(13.0))).clicked() {
                        start = true;
                    }
                }
                (_, Some(_)) => {
                    if message.is_some() {
                        // Before Argyll asks, there's nothing to answer yet
                        let can = self.prompt.is_some();
                        if ui.add_enabled(can, egui::Button::new(RichText::new("Measure").font(t.semibold(13.0)))).clicked() {
                            if let Some((_, job)) = self.job.as_mut() {
                                job.send_key(' ');
                            }
                            self.prompt = None;
                            self.placing = false;
                        }
                    }
                    if cancel_inline {
                        self.cancel_button(ui);
                    }
                }
                (Err(why), None) => {
                    ui.add_enabled(false, egui::Button::new("Begin"));
                    ui.label(RichText::new(why).color(t.caution));
                }
            }
            if let Some((ok, text)) = self.status.as_ref().filter(|_| status_inline) {
                ui.label(RichText::new(text).color(if *ok { t.good } else { t.bad }));
            }
        });
        if running.is_some_and(|k| k.step() == kind.step()) && message.is_none() && !self.adjust.active() {
            if let Some(stage) = running.and_then(JobKind::stage) {
                ui.add_space(4.0);
                ui.label(RichText::new(stage).color(t.text));
            }
            ui.add_space(6.0);
            self.progress_bar(ui);
        }
        start
    }

    fn progress_bar(&self, ui: &mut egui::Ui) {
        match self.progress {
            Some((n, m)) if m > 0 => {
                ui.add(egui::ProgressBar::new(n as f32 / m as f32).text(format!("Patch {n} of {m}")));
            }
            _ => {
                ui.add(egui::ProgressBar::new(0.0).animate(true).text("Working"));
            }
        }
    }

    fn log_view(&self, ui: &mut egui::Ui, max_height: f32) {
        let t = Tokens::get(ui.ctx());
        egui::Frame::new().fill(t.field).corner_radius(4).inner_margin(6).show(ui, |ui| {
            let mut area = ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).max_height(max_height);
            // A set height, not just a limit: the Verify page sizes its
            // patch list from what the log took, so a log that took only
            // the room left would never grow back
            if max_height.is_finite() {
                area = area.min_scrolled_height(max_height);
            }
            area.show(ui, |ui| {
                for line in &self.log {
                    ui.label(RichText::new(line).monospace().size(11.5));
                }
            });
        });
    }

    fn page_verify(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        if let Some(rx) = &self.verify_picker {
            if let Ok(picked) = rx.try_recv() {
                self.verify_picker = None;
                if picked.is_some() {
                    self.verify_profile = picked;
                }
            }
        }
        if let Some(rx) = &self.csv_picker {
            if let Ok(picked) = rx.try_recv() {
                self.csv_picker = None;
                if let (Some(path), Some(o)) = (picked, &self.verify_outcome) {
                    self.status = Some(match std::fs::write(&path, verify::csv(&verify::patches(), &o.results)) {
                        Ok(()) => (true, format!("Exported {}", path.display())),
                        Err(e) => (false, format!("Can't write {}: {e}", path.display())),
                    });
                }
            }
        }
        let running = self.job.is_some();
        ui.heading("Verify");
        Self::gap(ui);
        ui.label("Measures 57 patches through a profile's calibration and compares them with what the profile predicts.");
        Self::gap(ui);

        ui.horizontal(|ui| {
            ui.label("Profile");
            let label = self
                .verify_profile
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Choose a profile".into());
            ui.add_enabled_ui(!running, |ui| {
                egui::ComboBox::from_id_salt("verify-profile").width(320.0).selected_text(label).show_ui(ui, |ui| {
                    for p in self.saved_profiles() {
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.selectable_value(&mut self.verify_profile, Some(p), name);
                    }
                });
                if ui.add_enabled(self.verify_picker.is_none(), egui::Button::new("Browse…")).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let picked = rfd::FileDialog::new().set_title("Profile to verify").add_filter("ICC profiles", &["icc", "icm"]).pick_file();
                        let _ = tx.send(picked);
                        ctx.request_repaint();
                    });
                    self.verify_picker = Some(rx);
                }
            });
        });
        if let Some(p) = &self.verify_profile {
            if !p.with_extension("cal").is_file() {
                ui.label(RichText::new("No calibration file next to this profile, so it's measured with the calibration currently loaded.").color(t.caution));
            }
        }

        Self::gap(ui);
        if let Some((ok, text)) = &self.status {
            ui.label(RichText::new(text).color(if *ok { t.good } else { t.bad }));
            Self::gap(ui);
        }
        let measuring = self.job.as_ref().is_some_and(|(k, _)| *k == JobKind::Verify);
        if !measuring {
            if self.verify_profile.is_some() && self.job_controls(ui, JobKind::Verify, true, false) {
                let ctx = ui.ctx().clone();
                self.start_verification(&ctx);
            }
        } else {
            // While measuring: the placement prompt or the progress, then
            // Cancel under it
            if self.placing || self.prompt.is_some() {
                self.job_controls(ui, JobKind::Verify, false, false);
            } else {
                self.progress_bar(ui);
            }
            Self::gap(ui);
            self.cancel_button(ui);
        }

        let mut suggest_lut = false;
        if let Some(o) = &self.verify_outcome {
            let s = &o.summary;
            let color = match s.verdict {
                verify::Verdict::Good => t.good,
                verify::Verdict::Acceptable => t.caution,
                verify::Verdict::Poor => t.bad,
            };
            ui.add_space(10.0);
            egui::Frame::new().fill(t.inset).corner_radius(4).inner_margin(10).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(s.verdict.label()).font(t.semibold(18.0)).color(color));
                    let name = o.profile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.label(RichText::new(format!("{name}  ·  {}", profile_type(o.lut))).color(t.text_dim));
                });
                egui::Grid::new("verify-summary").num_columns(4).spacing([18.0, 4.0]).show(ui, |ui| {
                    ui.label("Average ΔE00");
                    ui.label(RichText::new(format!("{:.2}", s.average)).color(t.text));
                    ui.label("Maximum ΔE00");
                    ui.label(RichText::new(format!("{:.2}", s.maximum)).color(t.text));
                    ui.end_row();
                    ui.label("Grayscale average");
                    ui.label(RichText::new(format!("{:.2}", s.gray_average)).color(t.text));
                    ui.label("Grayscale maximum");
                    ui.label(RichText::new(format!("{:.2}", s.gray_maximum)).color(t.text));
                    ui.end_row();
                    ui.label("Best 90% average");
                    ui.label(RichText::new(format!("{:.2}", s.best90_average)).color(t.text));
                    ui.end_row();
                    if let Some(a) = o.additivity {
                        ui.label("Channel additivity");
                        ui.label(RichText::new(format!("off by {a:.2}%")).color(t.text)).on_hover_text(
                            "How closely white and the secondaries match the sum of their red, green and blue parts, \
                             as a percentage of white. A shaper+matrix profile assumes they match exactly.",
                        );
                        ui.end_row();
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "Good: average ≤ {} and maximum ≤ {}. Acceptable: ≤ {} and ≤ {}.",
                        verify::GOOD.0, verify::GOOD.1, verify::ACCEPTABLE.0, verify::ACCEPTABLE.1
                    ))
                    .small()
                    .color(t.text_dim),
                );
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Report: {}", o.report.display())).small().color(t.text_dim));
                    if ui.small_button("Open").clicked() {
                        system::current().open(o.report.as_os_str());
                    }
                    if ui.add_enabled(self.csv_picker.is_none(), egui::Button::new("Export CSV…").small()).clicked() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let ctx = ui.ctx().clone();
                        let dir = o.report.parent().map(PathBuf::from);
                        let name = o.report.with_extension("csv").file_name().map(|n| n.to_string_lossy().into_owned());
                        std::thread::spawn(move || {
                            let mut dialog = rfd::FileDialog::new().set_title("Export patches").add_filter("CSV", &["csv"]);
                            if let Some(d) = dir {
                                dialog = dialog.set_directory(d);
                            }
                            if let Some(n) = name {
                                dialog = dialog.set_file_name(n);
                            }
                            let _ = tx.send(dialog.save_file());
                            ctx.request_repaint();
                        });
                        self.csv_picker = Some(rx);
                    }
                });
                // Only a shaper+matrix result that isn't Good gets advice,
                // and which advice depends on whether the channels add up
                if !o.lut && s.verdict != verify::Verdict::Good {
                    if let Some(a) = o.additivity {
                        ui.add_space(6.0);
                        if a > verify::ADDITIVITY_LIMIT {
                            ui.label(RichText::new(format!(
                                "This display's channels don't add up (by {a:.1}% of white), which a shaper+matrix profile \
                                 can't describe. A LUT profile may verify better."
                            )).color(t.caution));
                            if ui.add_enabled(!running, egui::Button::new("Make a LUT profile…")).clicked() {
                                suggest_lut = true;
                            }
                        } else {
                            ui.label(RichText::new(format!(
                                "The display's channels add up (within {a:.1}% of white), so a LUT profile is unlikely to do \
                                 better. Calibrating again, or a correction for the display's backlight on the Spectro page, may."
                            )).color(t.text_dim));
                        }
                    }
                }
            });

            ui.add_space(6.0);
            let patches = verify::patches();
            // Fill the height the log leaves, so a taller window or a
            // closed log shows more patches
            let height = (ui.available_height() - self.verify_log_height - 8.0).max(80.0);
            ScrollArea::vertical().id_salt("verify-patches").auto_shrink([true, false]).max_height(height).show(ui, |ui| {
                egui::Grid::new("verify-table").num_columns(4).striped(true).spacing([10.0, 3.0]).show(ui, |ui| {
                    ui.label("");
                    ui.label(RichText::new("Patch").color(t.text_dim));
                    ui.label(RichText::new("ΔE00").color(t.text_dim));
                    ui.label("");
                    ui.end_row();
                    for r in &o.results {
                        let Some(p) = patches.get(r.index - 1) else { continue };
                        let [cr, cg, cb] = verify::swatch(p);
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 14.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2, egui::Color32::from_rgb(cr, cg, cb));
                        ui.label(&p.name);
                        let c = if r.de <= verify::GOOD.1 { t.good } else if r.de <= verify::ACCEPTABLE.1 { t.caution } else { t.bad };
                        ui.label(RichText::new(format!("{:.2}", r.de)).color(c));
                        // A bar scaled so 10 fills it
                        let (bar, _) = ui.allocate_exact_size(egui::vec2(120.0, 8.0), egui::Sense::hover());
                        ui.painter().rect_filled(bar, 2, t.field);
                        let mut fill = bar;
                        fill.set_width((bar.width() * (r.de / 10.0).min(1.0) as f32).max(1.0));
                        ui.painter().rect_filled(fill, 2, c);
                        ui.end_row();
                    }
                });
            });
        }

        // Switch Settings to a LUT and show them, for the next measurement
        if suggest_lut {
            self.settings.targets.profile = ProfileType::Lut;
            self.settings.targets.settle();
            self.save_settings();
            self.step = Step::Targets;
        }

        ui.add_space(8.0);
        let top = ui.cursor().top();
        egui::CollapsingHeader::new("Log").default_open(running).show(ui, |ui| self.log_view(ui, 180.0));
        // The patch list above was sized from last frame's height, so
        // redraw once when opening or closing the log changes it
        let used = ui.min_rect().bottom() - top;
        if (used - self.verify_log_height).abs() > 0.5 {
            self.verify_log_height = used;
            ui.ctx().request_repaint();
        }
    }
}

/// The DisplayCraft logo, `size` points square. The image has an opaque
/// dark background, so it's drawn as a rounded tile.
fn logo(size: f32) -> egui::Image<'static> {
    egui::Image::new(egui::include_image!("../assets/icons/hicolor/256x256/apps/displaycraft.png"))
        .fit_to_exact_size(egui::vec2(size, size))
        .corner_radius((size * 0.14) as u8)
}

/// A row of choices on the Settings page, spaced apart and wrapped when the
/// window is too narrow. A choice with its own value field goes in
/// `with_field`, so the two stay together.
fn choices(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(22.0, 6.0);
        add(ui);
    });
}

fn profile_type(lut: bool) -> &'static str {
    if lut { "LUT profile" } else { "shaper+matrix profile" }
}

/// A profile's graphs: calibration, tonal response and gamut
fn profile_charts(ui: &mut egui::Ui, name: &str, curves: Option<&profiles::Curves>) {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(name.trim_end_matches(".icc")).font(t.semibold(13.0)).color(t.text));
    let Some(c) = curves else {
        ui.label(RichText::new("This profile couldn't be read for graphs.").color(t.text_dim));
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(18.0, 12.0);
        match &c.calibration {
            Some(cal) => charts::calibration(cal).show(ui, 210.0, 225.0),
            None => {
                ui.label(RichText::new("No calibration curves").color(t.text_dim));
            }
        }
        if let Some(tone) = &c.tone {
            charts::tone(tone).show(ui, 210.0, 225.0);
        }
        if let Some(p) = c.primaries {
            charts::gamut(p, c.white).show(ui, 240.0, 260.0);
        }
    });
}

/// A live adjustment reading: the level, and for color adjustments the
/// chromaticity, the error and which way to turn each channel
fn reading_view(ui: &mut egui::Ui, r: &adjust::Reading) {
    let t = Tokens::get(ui.ctx());
    let arrow = |d: Dir| match d {
        Dir::Up => "▲",
        Dir::Down => "▼",
        Dir::Hold => "=",
    };
    let big = |text: String, color: egui::Color32| RichText::new(text).font(t.semibold(20.0)).color(color);
    egui::Grid::new("reading").num_columns(2).spacing([20.0, 8.0]).show(ui, |ui| {
        ui.label("Level");
        let level = match r.level_dir {
            Some(d) => format!("{:.1} cd/m²  {}", r.level, arrow(d)),
            None => format!("{:.1} cd/m²", r.level),
        };
        ui.label(big(level, t.text));
        ui.end_row();
        if let Some(((x, dx), (y, dy))) = r.xy {
            ui.label("Chromaticity");
            ui.label(big(format!("x {x:.4} {}   y {y:.4} {}", arrow(dx), arrow(dy)), t.text));
            ui.end_row();
        }
        if let Some(de) = r.de {
            ui.label("Off target");
            let color = if de <= 1.0 { t.good } else if de <= 3.0 { t.caution } else { t.bad };
            ui.label(big(format!("ΔE {de:.1}"), color));
            ui.end_row();
        }
        if let Some(rgb) = r.rgb {
            ui.label("Turn");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                for ((name, color), (dir, first)) in [("Red", egui::Color32::from_rgb(230, 80, 80)), ("Green", egui::Color32::from_rgb(90, 200, 100)), ("Blue", egui::Color32::from_rgb(100, 140, 255))]
                    .into_iter()
                    .zip(rgb)
                {
                    let text = format!("{name} {}", match dir {
                        Dir::Up => "up",
                        Dir::Down => "down",
                        Dir::Hold => "=",
                    });
                    let mut rt = RichText::new(text).font(t.semibold(if first { 18.0 } else { 15.0 })).color(color);
                    if first {
                        rt = rt.underline();
                    }
                    ui.label(rt).on_hover_text(if first { "Turn this one first" } else { "" });
                }
            });
            ui.end_row();
        }
    });
}

/// A circled question mark that explains `text` on hover
fn help(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    let size = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let color = if response.hovered() { t.text } else { t.text_dim };
    let painter = ui.painter();
    painter.circle_stroke(rect.center(), size * 0.45, egui::Stroke::new(1.2, color));
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(size * 0.72), color);
    response.on_hover_ui(|ui| {
        ui.set_max_width(360.0);
        ui.label(text);
    });
}

/// A choice and its value field, at normal spacing (child UIs inherit the
/// wider spacing of `choices`)
fn with_field(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        add(ui);
    });
}

/// The output a DisplayCraft profile was made for, from its name
/// ("eDP-1-20261012-204053.icc" -> "eDP-1"); None for other profiles
fn made_for(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".icc")?;
    let (rest, time) = stem.rsplit_once('-')?;
    let (output, date) = rest.rsplit_once('-')?;
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    (digits(date, 8) && digits(time, 6) && !output.is_empty()).then_some(output)
}

/// What to show for an Argyll prompt, answered with Measure (retry or
/// continue) or Cancel: a failed reading explained by what usually causes
/// it, or the prompt's own text without its key instructions, which don't
/// apply here. `before` is what Argyll printed just before it.
fn prompt_message(prompt: &str, before: &[String]) -> String {
    let said = |what: &str| before.iter().any(|l| l.contains(what));
    // A misread, an i1Display's diffuser left over the lens ("wrong
    // position"), or a calibration failing for either reason
    if said("misread") || said("wrong position") || said("Calibration failed") {
        return "Unable to measure. Check that the device is uncovered and placed on the measurement patch, then \
                press Measure to try again."
            .into();
    }
    if said("communication problem") {
        return "Lost contact with the measurement device. Check its connection, then press Measure to try again.".into();
    }
    if said("Place instrument back on test window") {
        return "Place the measurement device back on the patch, then press Measure.".into();
    }
    // Everything from the key instructions on, however they're worded
    let lower = prompt.to_ascii_lowercase();
    let cut = ["correct position then hit", "or hit esc", "hit esc", "hit any key", "any other key"]
        .iter()
        .filter_map(|k| lower.find(k))
        .min()
        .unwrap_or(prompt.len());
    let message = prompt[..cut].trim().trim_end_matches(',').trim();
    // Argyll prints "Place instrument on test window." as a line of its
    // own, so the placement prompt arrives as just the key instructions
    if message.is_empty() { "Place the measurement device on the patch and press Measure to begin.".into() } else { message.into() }
}

/// "/dev/bus/usb/001/017 (X-Rite i1 DisplayPro, ColorMunki Display)" ->
/// "i1Display Pro or ColorMunki Display"
fn instrument_name(description: &str) -> &str {
    let name = description.split_once(" (").and_then(|(_, rest)| rest.strip_suffix(')')).unwrap_or(description);
    // The one instrument Argyll lists under both names it's sold as. A
    // general ", " -> " or " would break others, e.g. "Q, Inc Prisma".
    match name {
        "X-Rite i1 DisplayPro, ColorMunki Display" => "i1Display Pro or ColorMunki Display",
        _ => name,
    }
}

/// "Monitor 1, Output eDP-1 (Sharp ...) at 0, 0, ..." -> "eDP-1"
fn output_name(description: &str) -> String {
    let name = description
        .split(", ")
        .find_map(|part| part.strip_prefix("Output "))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or("display");
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan();
        self.poll_job();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);

        egui::Panel::left("steps").exact_size(t.panel_w).resizable(false).show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.canvas).inner_margin(16)).show(ui, |ui| {
            if let Some(why) = self.unsupported().map(str::to_string).filter(|_| !self.step.always_open()) {
                self.page_unsupported(ui, &why);
                return;
            }
            let caution = match self.session.as_ref().map(|s| &s.platform) {
                Some(Platform::Unknown) => Some(
                    "This ArgyllCMS can't say whether it supports this session, so it's not the one DisplayCraft \
                     comes with: test patches and calibrations may be wrong. Choose the bundled ArgyllCMS in \
                     Preferences."
                        .to_string(),
                ),
                Some(Platform::Limited(why)) => Some(format!("Measurements may be wrong here. ArgyllCMS uses {why}.")),
                _ => None,
            };
            if let Some(text) = caution.filter(|_| !self.step.always_open()) {
                ui.label(RichText::new(text).color(t.caution));
                ui.add_space(8.0);
            }
            if let Some(err) = &self.inventory_error {
                ui.label(RichText::new(err).color(t.bad));
                ui.add_space(8.0);
            }
            match self.step {
                Step::Display => self.page_display(ui),
                // The instrument's display type list can be long (21 types
                // for an i1Display Pro), taller than the default window
                Step::Instrument => {
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.page_instrument(ui));
                }
                // The adjustment panel makes it taller than the window
                Step::Targets => {
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.page_targets(ui));
                }
                Step::Run => self.page_run(ui),
                Step::Verify => self.page_verify(ui),
                Step::Report => self.page_report(ui),
                Step::Profiles => {
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.page_profiles(ui));
                }
                Step::Preferences => self.page_preferences(ui),
                Step::About => {
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.page_about(ui));
                }
            }
        });
    }

    fn on_exit(&mut self) {
        // Don't leave a calibration running, with the display in its
        // measurement state, after the window closes
        if let Some((_, job)) = self.job.as_mut() {
            job.cancel();
        }
    }
}

#[cfg(test)]
mod hardware_tests {
    use std::time::{Duration, Instant};

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Step the UI until no tool is running, answering Argyll's prompts the
    /// way a user would.
    pub(super) fn wait_for_scan(harness: &mut Harness<'_, App>) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while harness.state().scan.is_some() {
            assert!(Instant::now() < deadline, "the device scan didn't finish");
            std::thread::sleep(Duration::from_millis(50));
            harness.step();
        }
        harness.run_steps(2);
    }

    pub(super) fn run_until_idle(harness: &mut Harness<'_, App>, limit: Duration) {
        let deadline = Instant::now() + limit;
        loop {
            harness.step();
            // The buttons only answer once Argyll has asked
            if harness.state().prompt.is_some() {
                for label in ["Measure", "Continue"] {
                    if let Some(button) = harness.query_by_label(label) {
                        button.click();
                    }
                }
            }
            if harness.state().job.is_none() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out; log:\n{}", harness.state().log.join("\n"));
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Calibrates and profiles at Low quality through the page with the
    /// real ArgyllCMS and instrument, without installing. Uses the saved
    /// Settings, and $DISPLAYCRAFT_TEST_LUT=1 for a LUT profile (at Medium,
    /// unless built with the low-quality-lut feature).
    #[test]
    #[ignore = "needs ArgyllCMS and an instrument on the screen"]
    fn calibrate_and_profile() {
        let mut harness = Harness::builder().with_size(egui::vec2(980.0, 640.0)).build_eframe(|cc| App::new(cc));
        wait_for_scan(&mut harness);
        harness.get_by_label_contains("Measure").click();
        harness.run_steps(2);
        let app = harness.state_mut();
        app.settings.targets.quality = Quality::Low;
        if std::env::var_os("DISPLAYCRAFT_TEST_LUT").is_some() {
            app.settings.targets.profile = ProfileType::Lut;
            app.settings.targets.settle();
        }
        println!("{}", app.settings.targets.describe());
        harness.run_steps(2);
        harness.get_by_label("Begin").click();
        harness.step();
        let started = Instant::now();
        run_until_idle(&mut harness, Duration::from_secs(3600));
        let app = harness.state();
        let o = app.outcome.as_ref().unwrap_or_else(|| panic!("no result: {:?}; log:\n{}", app.status, app.log.join("\n")));
        println!("{} s\n{}\nprofile {}", started.elapsed().as_secs(), o.summary.join("\n"), o.icc.display());
        let quick_left = std::fs::read_dir(App::data_dir().unwrap().join("work")).map(|d| d.count()).unwrap_or(0);
        assert_eq!(quick_left, 0, "the quick profile wasn't removed");
    }

    /// Clicks through a whole calibration with the real ArgyllCMS and
    /// instrument, then installs the profile.
    #[test]
    #[ignore = "needs ArgyllCMS, an instrument on the screen, and installs a profile"]
    fn calibrate_profile_and_install() {
        let mut harness = Harness::builder().with_size(egui::vec2(980.0, 640.0)).build_eframe(|cc| App::new(cc));
        wait_for_scan(&mut harness);
        assert!(harness.state().ready().is_ok(), "{:?}", harness.state().ready());

        harness.get_by_label_contains("Measure").click();
        harness.run_steps(2);
        // Low quality: the same path as the others, in less time
        harness.state_mut().settings.targets.quality = Quality::Low;
        harness.get_by_label("Begin").click();
        harness.step();
        assert!(harness.state().job.is_some(), "Begin didn't launch dispcal: {:?}", harness.state().status);

        run_until_idle(&mut harness, Duration::from_secs(1800));
        let app = harness.state();
        let outcome = app.outcome.as_ref().unwrap_or_else(|| panic!("no result; log:\n{}", app.log.join("\n")));
        println!("{}", outcome.summary.join("\n"));
        assert!(outcome.cal.is_file() && outcome.icc.is_file());

        harness.run_steps(2);
        harness.get_by_label("Install profile").click();
        harness.step();
        run_until_idle(&mut harness, Duration::from_secs(120));
        let app = harness.state();
        assert!(app.outcome.as_ref().is_some_and(|o| o.installed), "install failed; log:\n{}", app.log.join("\n"));
        println!("installed {}", app.outcome.as_ref().unwrap().icc.display());
    }
}

#[cfg(test)]
mod hardware_verify_tests {
    use std::time::Duration;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Verifies a profile through the Verify page with the real ArgyllCMS
    /// and instrument. Uses $DISPLAYCRAFT_TEST_PROFILE if set, else the
    /// newest saved profile. Measures only; changes no display settings.
    #[test]
    #[ignore = "needs ArgyllCMS, an instrument on the screen, and a saved profile"]
    fn verify_profile_through_ui() {
        let mut harness = Harness::builder().with_size(egui::vec2(980.0, 640.0)).build_eframe(|cc| App::new(cc));
        super::hardware_tests::wait_for_scan(&mut harness);
        if let Some(p) = std::env::var_os("DISPLAYCRAFT_TEST_PROFILE") {
            harness.state_mut().verify_profile = Some(PathBuf::from(p));
        }
        let profile = harness.state().verify_profile.clone().expect("a saved profile");
        println!("verifying {}", profile.display());

        harness.get_by_label_contains("Verify").click();
        harness.run_steps(2);
        harness.get_by_label("Begin").click();
        harness.step();
        assert!(harness.state().job.is_some(), "Begin didn't launch dispread: {:?}", harness.state().status);
        super::hardware_tests::run_until_idle(&mut harness, Duration::from_secs(900));

        let app = harness.state();
        let o = app.verify_outcome.as_ref().unwrap_or_else(|| panic!("no result: {:?}; log:\n{}", app.status, app.log.join("\n")));
        let s = &o.summary;
        println!(
            "{}: average {:.2}, maximum {:.2}, grayscale {:.2}/{:.2}, best 90% {:.2}\nreport {}",
            s.verdict.label(), s.average, s.maximum, s.gray_average, s.gray_maximum, s.best90_average, o.report.display()
        );
        assert_eq!(o.results.len(), verify::patches().len());
        assert!(o.report.is_file());
    }
}

#[cfg(test)]
mod hardware_report_tests {
    use std::time::Duration;

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Measures a display report through the Report page with the real
    /// ArgyllCMS and instrument, uncalibrated and as calibrated now.
    /// Measures only; changes no display settings.
    #[test]
    #[ignore = "needs ArgyllCMS and an instrument on the screen"]
    fn report_through_ui() {
        let mut harness = Harness::builder().with_size(egui::vec2(980.0, 640.0)).build_eframe(|cc| App::new(cc));
        super::hardware_tests::wait_for_scan(&mut harness);
        harness.get_by_label("Report").click();
        harness.run_steps(2);
        for mode in [ReportMode::Uncalibrated, ReportMode::Current] {
            harness.state_mut().report_mode = mode;
            harness.run_steps(2);
            harness.get_by_label("Begin").click();
            harness.step();
            assert!(harness.state().job.is_some(), "Begin didn't launch dispread: {:?}", harness.state().status);
            super::hardware_tests::run_until_idle(&mut harness, Duration::from_secs(300));
            let app = harness.state();
            let o = app.report_outcome.as_ref().unwrap_or_else(|| panic!("no report: {:?}; log:\n{}", app.status, app.log.join("\n")));
            println!("{}", std::fs::read_to_string(&o.saved).unwrap());
        }
    }
}

#[cfg(test)]
mod hardware_adjust_tests {
    use std::time::{Duration, Instant};

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Runs the display controls adjustment from the Settings page with the
    /// real ArgyllCMS and instrument: places the instrument, measures the
    /// white point for a few readings (pictured in tmp/page-adjusting.png),
    /// then finishes. Measures only.
    #[test]
    #[ignore = "needs ArgyllCMS, an instrument on the screen, and a GPU"]
    fn adjust_through_settings() {
        let mut harness = Harness::builder().with_size(egui::vec2(980.0, 640.0)).wgpu().build_eframe(|cc| App::new(cc));
        super::hardware_tests::wait_for_scan(&mut harness);
        harness.get_by_label("3  Settings").click();
        harness.run_steps(2);
        harness.get_by_label("Adjust…").click();
        harness.step();
        assert!(harness.state().job.is_some(), "Adjust didn't start dispcal: {:?}", harness.state().status);
        let wait = |harness: &mut Harness<'_, App>, what: &str, done: &dyn Fn(&App) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(120);
            while !done(harness.state()) {
                assert!(harness.state().job.is_some(), "dispcal ended while waiting for {what}; log:\n{}", harness.state().log.join("\n"));
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(50));
                harness.step();
            }
            harness.run_steps(2);
        };
        wait(&mut harness, "the placement prompt", &|a| a.prompt.is_some());
        harness.get_by_label("Measure").click();
        wait(&mut harness, "the menu", &|a| a.adjust.menu);
        // "White point" is also a row label on the page
        harness.get_by_role_and_label(egui::accesskit::Role::Button, "White point").click();
        wait(&mut harness, "readings", &|a| a.adjust.reading.is_some());
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            harness.step();
        }
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        harness.render().unwrap().save(dir.join("page-adjusting.png")).unwrap();
        println!("{:?}\n{:?}", harness.state().adjust.target, harness.state().adjust.reading);
        harness.get_by_label("Done").click();
        wait(&mut harness, "the menu again", &|a| a.adjust.menu);
        harness.get_by_label("Finish").click();
        super::hardware_tests::run_until_idle(&mut harness, Duration::from_secs(60));
        assert_eq!(harness.state().status, None, "log:\n{}", harness.state().log.join("\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::{display_name, instrument_name, output_name, prompt_message};

    #[test]
    fn profile_names() {
        assert_eq!(super::made_for("eDP-1-20261012-204053.icc"), Some("eDP-1"));
        assert_eq!(super::made_for("HDMI-A-1-20261012-204053.icc"), Some("HDMI-A-1"));
        assert_eq!(super::made_for("7JXK8-LQ170R1 #1 2024-09-08 15-17 2.2 F-S XYZLUT+MTX.icc"), None);
        assert_eq!(super::display_name("Monitor 2, Output DP-3 (Dell U2413) at 2560, 0, width 1920, height 1200"), "DP-3 (Dell U2413)");
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("Monitor 1, Output eDP-1 (Sharp Corporation eDP-1) at 0, 0, width 2560, height 1600"), "eDP-1 (Sharp Corporation eDP-1)");
        assert_eq!(display_name("Built-in Display, at 0, 0, width 2624, height 1696 (Primary Display)"), "Built-in Display");
    }

    #[test]
    fn prompt_messages() {
        let none: [String; 0] = [];
        let placing = prompt_message("Hit Esc or Q to give up, any other key to continue:", &none);
        assert_eq!(placing, "Place the measurement device on the patch and press Measure to begin.");
        let covered = ["Spot read failed due to the sensor being in the wrong position".to_string(), "(Instrument is in the wrong mode)".into()];
        let msg = prompt_message("Correct position then hit Esc or Q to give up, any other key to retry:", &covered);
        assert!(msg.starts_with("Unable to measure. Check that the device is uncovered"), "{msg}");
        let cal = ["Calibration failed with 'Misread' (...)".to_string()];
        assert!(prompt_message("Hit any key to retry, or Esc or Q to abort:", &cal).starts_with("Unable to measure"));
        assert_eq!(prompt_message("Something else happened, hit Esc or Q to abort: ", &none), "Something else happened");
    }

    #[test]
    fn instrument_name_from_description() {
        assert_eq!(instrument_name("/dev/bus/usb/001/017 (X-Rite i1 DisplayPro, ColorMunki Display)"), "i1Display Pro or ColorMunki Display");
        assert_eq!(instrument_name("/dev/ttyUSB0 (Q, Inc Prisma)"), "Q, Inc Prisma");
        assert_eq!(instrument_name("Spyder"), "Spyder");
    }

    #[test]
    fn output_name_from_description() {
        assert_eq!(output_name("Monitor 1, Output eDP-1 (Sharp Corporation eDP-1) at 0, 0, width 2560, height 1600"), "eDP-1");
        assert_eq!(output_name("Something else"), "display");
    }
}

#[cfg(test)]
mod page_pictures {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Renders each page to tmp/page-*.png for looking at, with made-up
    /// verification results.
    #[test]
    #[ignore = "needs ArgyllCMS and a GPU; writes pictures to tmp/"]
    fn render_pages() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let shoot = |step: Step, height: f32, log_open: bool, name: &str| {
            let mut harness = Harness::builder().with_size(egui::vec2(980.0, height)).wgpu().build_eframe(|cc| App::new(cc));
            super::hardware_tests::wait_for_scan(&mut harness);
            let app = harness.state_mut();
            app.step = step;
            app.settings.targets.white = White::D55;
            let patches = verify::patches();
            let results: Vec<verify::PatchResult> = (1..=patches.len())
                .map(|i| verify::PatchResult { index: i, expected_lab: [50.0, 0.0, 0.0], measured_lab: [50.0, 0.5, 0.5], de: (i % 7) as f64 * 0.4 })
                .collect();
            let summary = verify::summarize(&results, |i| i <= 21).unwrap();
            app.verify_outcome = Some(VerifyOutcome {
                profile: "/x/eDP-1-test.icc".into(),
                lut: false,
                additivity: Some(if name == "verify-suggest" { 4.8 } else { 0.26 }),
                summary,
                results,
                report: "/x/report.txt".into(),
            });
            app.log = vec!["$ dispread -v ...".into(), "Setting up the instrument".into()];
            // The report from this display's uncalibrated measurement
            let m: Vec<[f64; 3]> = report::PATCHES
                .iter()
                .map(|p| {
                    let l = |v: f64| (v / 100.0).powf(2.23);
                    let (r, g, b) = (l(p[0]), l(p[1]), l(p[2]));
                    [0.12 + 230.0 * (0.43 * r + 0.34 * g + 0.18 * b), 0.13 + 242.0 * (0.22 * r + 0.70 * g + 0.08 * b), 0.14 + 260.0 * (0.02 * r + 0.11 * g + 0.95 * b)]
                })
                .collect();
            app.report_outcome = Some(ReportOutcome {
                report: report::analyze(&m).unwrap(),
                title: "eDP-1, uncalibrated".into(),
                saved: "/x/eDP-1-report.txt".into(),
            });
            harness.run_steps(2);
            if name == "run-running" {
                let ctx = harness.ctx.clone();
                // Stands in for dispcal: the page shows what it shows while
                // waiting for the instrument to be placed
                let args = vec!["30".to_string()];
                harness.state_mut().start_job(&ctx, JobKind::Calibrate, std::path::Path::new("/usr/bin"), "sleep", &args);
                harness.run_steps(2);
            }
            if name == "verify-running" {
                let ctx = harness.ctx.clone();
                let args = vec!["30".to_string()];
                let app = harness.state_mut();
                app.start_job(&ctx, JobKind::Verify, std::path::Path::new("/usr/bin"), "sleep", &args);
                // Past the placement prompt, partway through
                app.placing = false;
                app.progress = Some((12, 57));
                harness.run_steps(2);
            }
            if name == "verify-status" {
                harness.state_mut().status = Some((true, "Profile installed. KWin now uses it for this display.".into()));
                harness.run_steps(2);
            }
            if name == "targets-lut" {
                let t = &mut harness.state_mut().settings.targets;
                t.profile = ProfileType::Lut;
                t.settle();
                harness.run_steps(2);
            }
            if name == "profiles-graphs" {
                harness.state_mut().profile_details = Some("eDP-1-20261012-204053.icc".into());
                harness.run_steps(2);
            }
            if name == "profiles-two" {
                let app = harness.state_mut();
                app.inventory.displays.push(argyll::Choice {
                    value: "2".into(),
                    description: "Monitor 2, Output DP-3 (Dell U2413) at 2560, 0, width 1920, height 1200".into(),
                });
                app.display = 1;
                app.profiles_view = None;
                harness.run_steps(2);
            }
            if name == "display-three" {
                let app = harness.state_mut();
                app.inventory.displays.push(argyll::Choice {
                    value: "2".into(),
                    description: "Monitor 2, Output DP-3 (Dell U2413) at 2048, 0, width 1200, height 1920".into(),
                });
                app.layout = layout::parse(
                    r#"{"outputs": [
                    {"name": "eDP-1", "enabled": true, "connected": true, "pos": {"x": 0, "y": 360}, "size": {"width": 2560, "height": 1600}, "scale": 1.25, "rotation": 1, "priority": 1, "replicationSource": 0},
                    {"name": "DP-3", "enabled": true, "connected": true, "pos": {"x": 2048, "y": 0}, "size": {"width": 1920, "height": 1200}, "scale": 1, "rotation": 2, "priority": 2, "replicationSource": 0},
                    {"name": "HDMI-A-1", "enabled": false, "connected": true, "pos": {"x": 0, "y": 0}, "size": {"width": 1920, "height": 1080}, "scale": 1, "rotation": 1, "priority": 0, "replicationSource": 0}]}"#,
                )
                .unwrap();
                app.display = 1;
                harness.run_steps(2);
                // Clicking a monitor selects it
                harness.get_by_label("eDP-1").click();
                harness.run_steps(2);
                assert_eq!(harness.state().display, 0, "clicking eDP-1 in the map didn't select it");
                harness.get_by_label("DP-3").click();
                harness.run_steps(2);
                assert_eq!(harness.state().display, 1);
            }
            if name == "display-open" {
                harness.state_mut().inventory.displays.push(argyll::Choice {
                    value: "2".into(),
                    description: "Monitor 2, Output DP-3 (Dell U2413) at 2560, 0, width 1920, height 1200".into(),
                });
                harness.run_steps(2);
                harness.get_by_value("eDP-1 (Sharp Corporation eDP-1)").click();
                harness.run_steps(4);
            }
            if name == "spectro-blocked" {
                // Whatever this machine allows, the real check runs too
                assert!(harness.state().blocked.is_empty(), "this machine's instrument is blocked: {:?}", harness.state().blocked);
                harness.state_mut().blocked =
                    vec![usbaccess::Blocked { name: "i1Display3".into(), node: "/dev/bus/usb/001/017".into() }];
                harness.run_steps(2);
            }
            if name == "spectro-open" {
                harness.get_by_value("None (use Instrument Default)").click();
                harness.run_steps(4);
            }
            if log_open {
                harness.get_by_label("Log").click();
                // Past the opening animation
                for _ in 0..30 {
                    harness.step();
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            harness.render().unwrap().save(dir.join(format!("page-{name}.png"))).unwrap();
            if let Some((_, job)) = harness.state_mut().job.as_mut() {
                job.cancel();
            }
        };
        shoot(Step::Display, 640.0, false, "display");
        shoot(Step::Display, 640.0, false, "display-three");
        shoot(Step::Display, 640.0, false, "display-open");
        shoot(Step::Instrument, 640.0, false, "spectro");
        shoot(Step::Instrument, 640.0, false, "spectro-blocked");
        shoot(Step::Instrument, 640.0, false, "spectro-open");
        shoot(Step::Targets, 640.0, false, "targets");
        shoot(Step::Targets, 640.0, false, "targets-lut");
        shoot(Step::Run, 640.0, false, "run");
        shoot(Step::Run, 640.0, false, "run-running");
        shoot(Step::Preferences, 640.0, false, "preferences");
        shoot(Step::About, 640.0, false, "about");
        shoot(Step::Verify, 640.0, false, "verify-status");
        shoot(Step::Verify, 640.0, false, "verify-running");
        shoot(Step::Verify, 800.0, false, "verify-suggest");
        shoot(Step::Verify, 640.0, false, "verify-640");
        shoot(Step::Verify, 900.0, false, "verify-900");
        shoot(Step::Verify, 900.0, true, "verify-900-log");
        shoot(Step::Report, 640.0, false, "report");
        shoot(Step::Profiles, 640.0, false, "profiles");
        shoot(Step::Profiles, 900.0, false, "profiles-graphs");
        shoot(Step::Profiles, 640.0, false, "profiles-two");
    }
}
