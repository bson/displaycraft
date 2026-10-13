//! The calibration workflow: pick a display and instrument, set targets, then
//! run dispcal to calibrate and make a matrix profile, and install it.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use egui::{Align, Layout, RichText, ScrollArea};

use crate::argyll::{self, Event, Inventory, Job};
use crate::corrections::{self, Correction};
use crate::settings::Settings;
use crate::targets::{Quality, Tone, White};
use crate::verify;
use crate::theme::{self, ThemeKind, Tokens};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Display,
    Instrument,
    Targets,
    Run,
    Verify,
}

impl Step {
    const ALL: [Step; 5] = [Step::Display, Step::Instrument, Step::Targets, Step::Run, Step::Verify];

    fn label(self) -> &'static str {
        match self {
            Step::Display => "Display",
            Step::Instrument => "Spectro",
            Step::Targets => "Settings",
            Step::Run => "Measure",
            Step::Verify => "Verify",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    Calibrate,
    Install,
    Verify,
}

impl JobKind {
    /// The page that shows this job
    fn step(self) -> Step {
        match self {
            JobKind::Calibrate | JobKind::Install => Step::Run,
            JobKind::Verify => Step::Verify,
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
    summary: verify::Summary,
    results: Vec<verify::PatchResult>,
    report: PathBuf,
}

/// What the last calibration produced.
struct Outcome {
    cal: PathBuf,
    icc: PathBuf,
    /// dispcal's own summary of how close it got, e.g. "White point error = 0.35 deltaE"
    summary: Vec<String>,
    installed: bool,
}

/// The Preferences window's unapplied state.
struct Prefs {
    custom: bool,
    path: String,
    /// The last check of `path`: ArgyllCMS version, or what's wrong.
    check: Option<Result<String, String>>,
    /// A folder picker running on its own thread.
    picker: Option<Receiver<Option<PathBuf>>>,
}

impl Prefs {
    fn from_settings(settings: &Settings) -> Prefs {
        let mut prefs = Prefs {
            custom: settings.argyll_bin.is_some(),
            path: settings
                .argyll_bin
                .clone()
                .or_else(argyll::auto_bin_dir)
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            check: None,
            picker: None,
        };
        prefs.recheck();
        prefs
    }

    fn chosen(&self) -> Option<PathBuf> {
        let path = self.path.trim();
        (self.custom && !path.is_empty()).then(|| PathBuf::from(path))
    }

    fn recheck(&mut self) {
        let dir = if self.custom { self.chosen() } else { argyll::auto_bin_dir() };
        self.check = Some(match dir {
            Some(dir) => argyll::probe(&dir),
            None => Err("ArgyllCMS wasn't found".into()),
        });
    }
}

pub struct App {
    settings: Settings,
    prefs: Option<Prefs>,
    bin: Option<PathBuf>,
    /// What ArgyllCMS says about this session (None until it's been found)
    platform: Option<argyll::Platform>,
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
    /// From Start until the user says the instrument is in place: the first
    /// prompt (Argyll's "Place instrument on test window") is answered by
    /// the Measure button rather than shown
    placing: bool,
    /// Cancel was clicked for the running job. Argyll exits with status 0
    /// when signalled, so this is how a cancelled run is told from a
    /// finished one.
    cancelled: bool,
    status: Option<(bool, String)>,
    pending: Option<(PathBuf, PathBuf)>,
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
        let mut app = App {
            bin: argyll::find_bin_dir(settings.argyll_bin.as_deref()),
            settings,
            prefs: None,
            platform: None,
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
            pending: None,
            outcome: None,
            corrections: corrections::find(),
            correction_picker: None,
            verify_profile: None,
            verify_picker: None,
            verify_run: None,
            verify_outcome: None,
            csv_picker: None,
            verify_log_height: 30.0,
        };
        app.refresh_inventory();
        app.verify_profile = app.saved_profiles().into_iter().next();
        app
    }

    /// Identify what can be measured: first whether ArgyllCMS supports this
    /// session at all, then the displays and instruments.
    fn refresh_inventory(&mut self) {
        let Some(bin) = &self.bin else {
            self.platform = None;
            self.inventory = Inventory::default();
            self.inventory_error = Some("ArgyllCMS wasn't found. Set its directory in Preferences.".into());
            return;
        };
        let platform = argyll::check_platform(bin);
        let unsupported = matches!(platform, argyll::Platform::Unsupported(_));
        self.platform = Some(platform);
        if unsupported {
            self.inventory = Inventory::default();
            self.inventory_error = None;
            return;
        }
        match argyll::inventory(bin) {
            Ok(inv) => {
                self.display = self.display.min(inv.displays.len().saturating_sub(1));
                self.instrument = self.instrument.min(inv.instruments.len().saturating_sub(1));
                self.display_type = self.display_type.filter(|&i| i < inv.display_types.len());
                self.inventory = inv;
                self.inventory_error = None;
            }
            Err(e) => {
                self.inventory = Inventory::default();
                self.inventory_error =
                    Some(format!("Couldn't run dispcal in {}: {e}. Check the ArgyllCMS directory in Preferences.", bin.display()));
            }
        }
    }

    fn unsupported(&self) -> Option<&str> {
        match &self.platform {
            Some(argyll::Platform::Unsupported(why)) => Some(why),
            _ => None,
        }
    }

    fn ready(&self) -> Result<(), &'static str> {
        if self.unsupported().is_some() {
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
        let display = &self.inventory.displays[self.display];
        let t = &self.settings.targets;
        let mut args: Vec<String> = vec!["-v".into(), "-d".into(), display.value.clone()];
        args.extend(self.instrument_args());
        // Skip the interactive monitor-controls adjustment
        args.push("-m".into());
        args.extend(["-q".into(), t.quality.arg().into()]);
        if let Some(k) = t.kelvin() {
            args.extend(["-t".into(), format!("{k:.0}")]);
        }
        args.extend(["-g".into(), t.gamma_arg()]);
        if t.set_brightness {
            args.extend(["-b".into(), format!("{:.0}", t.brightness)]);
        }
        // -o takes an optional file name, so give it one: otherwise it
        // would take the output base name that follows
        let icc = base.with_extension("icc");
        args.extend(["-o".into(), icc.display().to_string(), base.display().to_string()]);

        self.start_job(ctx, JobKind::Calibrate, &bin, "dispcal", &args);
        self.pending = Some((base.with_extension("cal"), icc));
        self.outcome = None;
    }

    /// The chosen display, e.g. "eDP-1 (Sharp Corporation eDP-1)"
    fn display_label(&self) -> Option<String> {
        let d = self.inventory.displays.get(self.display)?;
        // Argyll's description leads with "Monitor 1, Output "
        let text = d.description.split(", ").find_map(|p| p.strip_prefix("Output ")).unwrap_or(&d.description);
        Some(text.split(" at ").next().unwrap_or(text).to_string())
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
        let report = run.base.with_extension("txt");
        let text = verify::report(&run.profile, &summary, &verify::patches(), &results, run.correction.as_deref());
        std::fs::write(&report, text).map_err(|e| format!("Can't write the report: {e}"))?;
        Ok(VerifyOutcome { profile: run.profile, summary, results, report })
    }

    fn install_profile(&mut self, ctx: &egui::Context) {
        let (Some(bin), Some(outcome)) = (self.bin.clone(), self.outcome.as_ref()) else { return };
        let display = &self.inventory.displays[self.display];
        let args = vec!["-d".into(), display.value.clone(), "-I".into(), outcome.icc.display().to_string()];
        self.start_job(ctx, JobKind::Install, &bin, "dispwin", &args);
    }

    fn start_job(&mut self, ctx: &egui::Context, kind: JobKind, bin: &std::path::Path, tool: &str, args: &[String]) {
        self.log.clear();
        self.log.push(format!("$ {tool} {}", args.join(" ")));
        self.progress = None;
        self.prompt = None;
        self.placing = kind != JobKind::Install;
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
                Event::Line(line) => self.log.push(line),
                Event::Progress(n, m) => self.progress = Some((n, m)),
                Event::Prompt(text) => self.prompt = Some(text),
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
            JobKind::Calibrate => {
                let pending = self.pending.take();
                if cancelled {
                    // Whatever dispcal got as far as writing is incomplete
                    if let Some((cal, icc)) = &pending {
                        for f in [cal, icc, &cal.with_extension("ti3")] {
                            let _ = std::fs::remove_file(f);
                        }
                    }
                    self.status = None;
                } else if let (true, Some((cal, icc))) = (ok, pending.filter(|(cal, icc)| cal.is_file() && icc.is_file())) {
                    let summary = self
                        .log
                        .iter()
                        .filter(|l| l.contains("error =") || l.starts_with("White point error") || l.contains("neutral error"))
                        .cloned()
                        .collect::<Vec<_>>();
                    // dispcal prints its summary twice (after calibrating
                    // and after verifying); keep the final one
                    let summary = summary.iter().rev().take(4).rev().cloned().collect();
                    self.outcome = Some(Outcome { cal, icc, summary, installed: false });
                    self.status = Some((true, "Calibration and profile done".into()));
                } else {
                    self.status = Some((false, "Calibration didn't finish. See the log.".into()));
                }
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.horizontal(|ui| {
            ui.label(RichText::new(concat!("DisplayCraft ", env!("CARGO_PKG_VERSION"))).font(t.semibold(15.0)).color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Preferences").clicked() && self.prefs.is_none() {
                    self.prefs = Some(Prefs::from_settings(&self.settings));
                }
            });
        });
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

    fn preferences(&mut self, ctx: &egui::Context) {
        let Some(mut prefs) = self.prefs.take() else { return };
        let t = Tokens::get(ctx);
        let running = self.job.is_some();

        // Folder picker result from its thread
        if let Some(rx) = &prefs.picker {
            if let Ok(picked) = rx.try_recv() {
                prefs.picker = None;
                if let Some(dir) = picked {
                    prefs.custom = true;
                    prefs.path = dir.display().to_string();
                    prefs.recheck();
                }
            }
        }

        let mut open = true;
        let mut apply = false;
        let mut theme = self.theme;
        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(logo(36.0));
                    ui.label(RichText::new("Settings are saved when applied.").color(t.text_dim));
                });
                ui.add_space(8.0);
                ui.label(RichText::new("ArgyllCMS programs").font(t.semibold(13.0)).color(t.text));
                ui.add_space(2.0);
                let auto = argyll::auto_bin_dir().map(|d| d.display().to_string()).unwrap_or_else(|| "not found".into());
                if ui.radio(!prefs.custom, format!("Find automatically ({auto})")).clicked() {
                    prefs.custom = false;
                    prefs.recheck();
                }
                if ui.radio(prefs.custom, "Use this directory:").clicked() {
                    prefs.custom = true;
                    prefs.recheck();
                }
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    let edit = ui.add_enabled(
                        prefs.custom,
                        egui::TextEdit::singleline(&mut prefs.path).desired_width(300.0).hint_text("/opt/argyll-kde-wayland/bin"),
                    );
                    if edit.lost_focus() {
                        prefs.recheck();
                    }
                    let browsing = prefs.picker.is_some();
                    if ui.add_enabled(!browsing, egui::Button::new("Browse…")).clicked() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let start = prefs.chosen().filter(|p| p.is_dir());
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
                match &prefs.check {
                    Some(Ok(version)) => ui.label(RichText::new(format!("Found ArgyllCMS {version}")).color(t.good)),
                    Some(Err(why)) => ui.label(RichText::new(why).color(t.bad)),
                    None => ui.label(""),
                };

                ui.add_space(12.0);
                ui.label(RichText::new("Appearance").font(t.semibold(13.0)).color(t.text));
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    egui::ComboBox::from_id_salt("theme").selected_text(theme.label()).show_ui(ui, |ui| {
                        for k in ThemeKind::ALL {
                            ui.selectable_value(&mut theme, k, k.label());
                        }
                    });
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let ok = prefs.check.as_ref().is_some_and(|c| c.is_ok());
                    let button = ui.add_enabled(!running && ok, egui::Button::new(RichText::new("Apply").font(t.semibold(13.0))));
                    if button.clicked() {
                        apply = true;
                    }
                    if running {
                        ui.label(RichText::new("Can't change while a calibration runs").color(t.text_dim));
                    }
                });
            });

        // The theme applies at once, so it can be judged by eye
        self.set_theme(ctx, theme);

        if apply {
            self.settings.argyll_bin = prefs.chosen();
            self.save_settings();
            self.bin = argyll::find_bin_dir(self.settings.argyll_bin.as_deref());
            self.refresh_inventory();
        }
        if open && !apply {
            self.prefs = Some(prefs);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.add_space(12.0);
        ui.vertical_centered(|ui| ui.add(logo(112.0)));
        ui.add_space(10.0);
        let running = self.job.is_some();
        for (i, step) in Step::ALL.into_iter().enumerate() {
            let selected = self.step == step;
            let text = RichText::new(format!("{}  {}", i + 1, step.label())).color(if selected { t.text } else { t.text_label });
            let button = egui::Button::new(text).selected(selected).min_size(egui::vec2(ui.available_width(), 30.0));
            // Settings can't change under a running calibration
            let job_step = self.job.as_ref().map(|(kind, _)| kind.step());
            let usable = self.unsupported().is_none();
            if ui.add_enabled(usable && (!running || Some(step) == job_step), button).clicked() {
                self.step = step;
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(8.0);
            if let Some(bin) = &self.bin {
                ui.label(RichText::new(bin.display().to_string()).small().color(t.text_dim));
            }
            if !self.inventory.version.is_empty() {
                ui.label(RichText::new(format!("ArgyllCMS {}", self.inventory.version)).small().color(t.text_dim));
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
        ui.add_space(6.0);
        ui.label(
            RichText::new("DisplayCraft needs KDE Plasma on Wayland, with the Wayland build of ArgyllCMS.").color(t.text_dim),
        );
        if let Some(bin) = &self.bin {
            ui.label(RichText::new(format!("Checked with {}", bin.join("dispwin").display())).small().color(t.text_dim));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Check again").clicked() {
                self.refresh_inventory();
            }
            if ui.button("Preferences").clicked() && self.prefs.is_none() {
                self.prefs = Some(Prefs::from_settings(&self.settings));
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
        if self.inventory.displays.is_empty() {
            ui.label("No displays found.");
        }
        for (i, d) in self.inventory.displays.iter().enumerate() {
            ui.radio_value(&mut self.display, i, &d.description);
        }
        Self::gap(ui);
        if ui.button("Refresh").clicked() {
            self.refresh_inventory();
        }
    }

    fn page_instrument(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Spectrophotometer or Colorimeter");
        Self::gap(ui);
        ui.label("Select the measurement device to use.");
        Self::gap(ui);
        if self.inventory.instruments.is_empty() {
            ui.label(RichText::new("No instrument found. Connect one, then refresh.").color(t.caution));
        }
        for (i, inst) in self.inventory.instruments.iter().enumerate() {
            ui.radio_value(&mut self.instrument, i, instrument_name(&inst.description)).on_hover_text(&inst.description);
        }
        Self::gap(ui);
        if ui.button("Refresh").clicked() {
            self.refresh_inventory();
        }
        self.corrections_section(ui);
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
        Self::gap(ui);
        ui.label("What the calibration should aim for.");
        Self::gap(ui);
        let before = self.settings.targets.clone();
        let tg = &mut self.settings.targets;
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
                ui.radio_value(&mut tg.quality, Quality::Low, "Low");
                ui.radio_value(&mut tg.quality, Quality::Medium, "Medium");
                ui.radio_value(&mut tg.quality, Quality::High, "High");
            });
            ui.end_row();
        });
        Self::gap(ui);
        ui.label(RichText::new("A higher quality setting requires more measurements and will take longer to perform.").color(t.text_dim));
        if self.settings.targets != before {
            self.save_settings();
        }
    }

    fn page_run(&mut self, ui: &mut egui::Ui) {
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
        if self.job_controls(ui, JobKind::Calibrate) {
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

    /// Start, and while a job runs, what it's waiting for and Cancel.
    /// Returns true when Start is clicked. Argyll's first prompt asks for
    /// the instrument to be placed, so a placement message shows from Start
    /// on, before the prompt itself arrives; Measure answers it. Prompts
    /// with text of their own (e.g. to retry after a failed reading) show
    /// it.
    fn job_controls(&mut self, ui: &mut egui::Ui, kind: JobKind) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut start = false;
        let running = self.job.as_ref().map(|(k, _)| *k);
        let message = match self.prompt.as_deref() {
            Some(p) => Some(prompt_message(p)),
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
                    if ui.button(RichText::new("Start").font(t.semibold(13.0))).clicked() {
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
                    if ui.add_enabled(!self.cancelled, egui::Button::new("Cancel")).clicked() {
                        if let Some((_, job)) = self.job.as_mut() {
                            job.cancel();
                        }
                        self.cancelled = true;
                    }
                }
                (Err(why), None) => {
                    ui.add_enabled(false, egui::Button::new("Start"));
                    ui.label(RichText::new(why).color(t.caution));
                }
            }
            if let Some((ok, text)) = &self.status {
                ui.label(RichText::new(text).color(if *ok { t.good } else { t.bad }));
            }
        });
        if running.is_some_and(|k| k.step() == kind.step()) && message.is_none() {
            self.progress_bar(ui);
        }
        start
    }

    fn progress_bar(&self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
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
        if self.verify_profile.is_some() && self.job_controls(ui, JobKind::Verify) {
            let ctx = ui.ctx().clone();
            self.start_verification(&ctx);
        }

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
                    ui.label(RichText::new(o.profile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).color(t.text_dim));
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
                        let _ = std::process::Command::new("xdg-open").arg(&o.report).spawn();
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
    egui::Image::new(egui::include_image!("../assets/displaycraft-256.png"))
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

/// A choice and its value field, at normal spacing (child UIs inherit the
/// wider spacing of `choices`)
fn with_field(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        add(ui);
    });
}

/// Argyll's prompt without its key instructions, which don't apply here
fn prompt_message(text: &str) -> String {
    let message = text
        .replace("Hit Esc or Q to give up, any other key to continue:", "")
        .replace("Hit Esc or Q to give up, any other key to retry:", "")
        .trim()
        .to_string();
    // Argyll prints "Place instrument on test window." as a line of its
    // own, so the placement prompt arrives as just the key instructions
    if message.is_empty() { "Place the measurement device on the patch and press Measure to begin.".into() } else { message }
}

/// "/dev/bus/usb/001/017 (X-Rite i1 DisplayPro, ColorMunki Display)" ->
/// "X-Rite i1 DisplayPro, ColorMunki Display"
fn instrument_name(description: &str) -> &str {
    description
        .split_once(" (")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or(description)
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
        self.poll_job();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);

        egui::Panel::top("top").exact_size(42.0).show(ui, |ui| {
            ui.add_space(8.0);
            self.top_bar(ui);
        });
        egui::Panel::left("steps").exact_size(t.panel_w).resizable(false).show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.canvas).inner_margin(16)).show(ui, |ui| {
            if let Some(why) = self.unsupported().map(str::to_string) {
                self.page_unsupported(ui, &why);
                return;
            }
            if self.platform == Some(argyll::Platform::Unknown) {
                ui.label(
                    RichText::new(
                        "This ArgyllCMS can't say whether it supports this platform, so it's probably not the \
                         Wayland build: test patches and calibrations may be wrong. Set the Wayland build's \
                         directory in Preferences.",
                    )
                    .color(t.caution),
                );
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
                Step::Targets => self.page_targets(ui),
                Step::Run => self.page_run(ui),
                Step::Verify => self.page_verify(ui),
            }
        });
        self.preferences(&ctx);
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

    /// Clicks through a whole calibration with the real ArgyllCMS and
    /// instrument, then installs the profile.
    #[test]
    #[ignore = "needs ArgyllCMS, an instrument on the screen, and installs a profile"]
    fn calibrate_profile_and_install() {
        let mut harness = Harness::builder().with_size(egui::vec2(980.0, 640.0)).build_eframe(|cc| App::new(cc));
        harness.run_steps(2);
        assert!(harness.state().ready().is_ok(), "{:?}", harness.state().ready());

        harness.get_by_label_contains("Measure").click();
        harness.run_steps(2);
        // Low quality: the same path as the others, in less time
        harness.state_mut().settings.targets.quality = Quality::Low;
        harness.get_by_label("Start").click();
        harness.step();
        assert!(harness.state().job.is_some(), "Start didn't launch dispcal: {:?}", harness.state().status);

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
        harness.run_steps(2);
        if let Some(p) = std::env::var_os("DISPLAYCRAFT_TEST_PROFILE") {
            harness.state_mut().verify_profile = Some(PathBuf::from(p));
        }
        let profile = harness.state().verify_profile.clone().expect("a saved profile");
        println!("verifying {}", profile.display());

        harness.get_by_label_contains("Verify").click();
        harness.run_steps(2);
        harness.get_by_label("Start").click();
        harness.step();
        assert!(harness.state().job.is_some(), "Start didn't launch dispread: {:?}", harness.state().status);
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
mod tests {
    use super::{instrument_name, output_name};

    #[test]
    fn instrument_name_from_description() {
        assert_eq!(instrument_name("/dev/bus/usb/001/017 (X-Rite i1 DisplayPro, ColorMunki Display)"), "X-Rite i1 DisplayPro, ColorMunki Display");
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
            harness.run_steps(2);
            let app = harness.state_mut();
            app.step = step;
            app.settings.targets.white = White::D55;
            let patches = verify::patches();
            let results: Vec<verify::PatchResult> = (1..=patches.len())
                .map(|i| verify::PatchResult { index: i, expected_lab: [50.0, 0.0, 0.0], measured_lab: [50.0, 0.5, 0.5], de: (i % 7) as f64 * 0.4 })
                .collect();
            let summary = verify::summarize(&results, |i| i <= 21).unwrap();
            app.verify_outcome = Some(VerifyOutcome { profile: "/x/eDP-1-test.icc".into(), summary, results, report: "/x/report.txt".into() });
            app.log = vec!["$ dispread -v ...".into(), "Setting up the instrument".into()];
            harness.run_steps(2);
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
        };
        shoot(Step::Display, 640.0, false, "display");
        shoot(Step::Instrument, 640.0, false, "spectro");
        shoot(Step::Instrument, 640.0, false, "spectro-open");
        shoot(Step::Targets, 640.0, false, "targets");
        shoot(Step::Run, 640.0, false, "run");
        shoot(Step::Verify, 640.0, false, "verify-640");
        shoot(Step::Verify, 900.0, false, "verify-900");
        shoot(Step::Verify, 900.0, true, "verify-900-log");
    }
}
