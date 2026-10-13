//! The calibration workflow: pick a display and instrument, set targets, then
//! run dispcal to calibrate and make a matrix profile, and install it.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use egui::{Align, Layout, RichText, ScrollArea};

use crate::argyll::{self, Event, Inventory, Job};
use crate::settings::Settings;
use crate::theme::{self, ThemeKind, Tokens};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Display,
    Instrument,
    Targets,
    Run,
}

impl Step {
    const ALL: [Step; 4] = [Step::Display, Step::Instrument, Step::Targets, Step::Run];

    fn label(self) -> &'static str {
        match self {
            Step::Display => "Display",
            Step::Instrument => "Instrument",
            Step::Targets => "Targets",
            Step::Run => "Calibrate & profile",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum White {
    Native,
    Daylight,
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Gamma,
    Srgb,
}

#[derive(Clone, Copy, PartialEq)]
enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    fn arg(self) -> &'static str {
        match self {
            Quality::Low => "l",
            Quality::Medium => "m",
            Quality::High => "h",
        }
    }
}

struct Targets {
    white: White,
    kelvin: f64,
    tone: Tone,
    gamma: f64,
    set_brightness: bool,
    brightness: f64,
    quality: Quality,
}

impl Default for Targets {
    fn default() -> Self {
        Targets {
            white: White::Daylight,
            kelvin: 6500.0,
            tone: Tone::Gamma,
            gamma: 2.2,
            set_brightness: false,
            brightness: 120.0,
            quality: Quality::Medium,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    Calibrate,
    Install,
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
    inventory: Inventory,
    inventory_error: Option<String>,
    theme: ThemeKind,
    step: Step,
    display: usize,
    instrument: usize,
    display_type: Option<usize>,
    targets: Targets,
    job: Option<(JobKind, Job)>,
    log: Vec<String>,
    progress: Option<(u32, u32)>,
    prompt: Option<String>,
    status: Option<(bool, String)>,
    pending: Option<(PathBuf, PathBuf)>,
    outcome: Option<Outcome>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
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
            inventory: Inventory::default(),
            inventory_error: None,
            theme,
            step: Step::Display,
            display: 0,
            instrument: 0,
            display_type: None,
            targets: Targets::default(),
            job: None,
            log: Vec::new(),
            progress: None,
            prompt: None,
            status: None,
            pending: None,
            outcome: None,
        };
        app.refresh_inventory();
        app
    }

    fn refresh_inventory(&mut self) {
        let Some(bin) = &self.bin else {
            self.inventory = Inventory::default();
            self.inventory_error = Some("ArgyllCMS wasn't found. Set its directory in Preferences.".into());
            return;
        };
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

    fn ready(&self) -> Result<(), &'static str> {
        if self.inventory.displays.is_empty() {
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
        let instrument = &self.inventory.instruments[self.instrument];
        let t = &self.targets;
        let mut args: Vec<String> = vec!["-v".into(), "-d".into(), display.value.clone(), "-c".into(), instrument.value.clone()];
        if let Some(dt) = self.display_type.and_then(|i| self.inventory.display_types.get(i)) {
            args.extend(["-y".into(), dt.value.clone()]);
        }
        // Skip the interactive monitor-controls adjustment
        args.push("-m".into());
        args.extend(["-q".into(), t.quality.arg().into()]);
        if t.white == White::Daylight {
            args.extend(["-t".into(), format!("{:.0}", t.kelvin)]);
        }
        match t.tone {
            Tone::Gamma => args.extend(["-g".into(), format!("{:.2}", t.gamma)]),
            Tone::Srgb => args.extend(["-g".into(), "s".into()]),
        }
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
        let ok = code == Some(0);
        match kind {
            JobKind::Calibrate => {
                let pending = self.pending.take();
                if let (true, Some((cal, icc))) = (ok, pending) {
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
            JobKind::Install => {
                if ok {
                    if let Some(o) = self.outcome.as_mut() {
                        o.installed = true;
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
            ui.label(RichText::new("displaycraft").font(t.semibold(15.0)).color(t.text));
            if !self.inventory.version.is_empty() {
                ui.label(RichText::new(format!("ArgyllCMS {}", self.inventory.version)).small().color(t.text_dim));
            }
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
        ui.add_space(8.0);
        let running = self.job.is_some();
        for (i, step) in Step::ALL.into_iter().enumerate() {
            let selected = self.step == step;
            let text = RichText::new(format!("{}  {}", i + 1, step.label())).color(if selected { t.text } else { t.text_label });
            let button = egui::Button::new(text).selected(selected).min_size(egui::vec2(ui.available_width(), 30.0));
            // Settings can't change under a running calibration
            if ui.add_enabled(!running || step == Step::Run, button).clicked() {
                self.step = step;
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(8.0);
            if let Some(bin) = &self.bin {
                ui.label(RichText::new(bin.display().to_string()).small().color(t.text_dim));
            }
        });
    }

    fn page_display(&mut self, ui: &mut egui::Ui) {
        ui.heading("Display");
        ui.label("The display to calibrate. Test patches appear in the middle of it.");
        ui.add_space(8.0);
        if self.inventory.displays.is_empty() {
            ui.label("No displays found.");
        }
        for (i, d) in self.inventory.displays.iter().enumerate() {
            ui.radio_value(&mut self.display, i, &d.description);
        }
        ui.add_space(8.0);
        if ui.button("Refresh").clicked() {
            self.refresh_inventory();
        }
    }

    fn page_instrument(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Instrument");
        ui.label("The colorimeter or spectrometer to measure with.");
        ui.add_space(8.0);
        if self.inventory.instruments.is_empty() {
            ui.label(RichText::new("No instrument found. Connect one, then refresh.").color(t.caution));
        }
        for (i, inst) in self.inventory.instruments.iter().enumerate() {
            ui.radio_value(&mut self.instrument, i, &inst.description);
        }
        if ui.button("Refresh").clicked() {
            self.refresh_inventory();
        }
        if !self.inventory.display_types.is_empty() {
            ui.add_space(12.0);
            ui.label(RichText::new("Display type").font(t.semibold(13.0)).color(t.text));
            ui.label(RichText::new("Matches the instrument's correction to the display's backlight.").color(t.text_dim));
            ui.radio_value(&mut self.display_type, None, "Instrument default");
            for (i, dt) in self.inventory.display_types.iter().enumerate() {
                ui.radio_value(&mut self.display_type, Some(i), format!("{} ({})", dt.description, dt.value));
            }
        }
    }

    fn page_targets(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        let tg = &mut self.targets;
        ui.heading("Targets");
        ui.add_space(8.0);
        egui::Grid::new("targets").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label("White point");
            ui.horizontal(|ui| {
                ui.radio_value(&mut tg.white, White::Daylight, "Daylight");
                ui.add_enabled(tg.white == White::Daylight, egui::DragValue::new(&mut tg.kelvin).range(3000.0..=10000.0).speed(10.0).suffix(" K"));
                ui.radio_value(&mut tg.white, White::Native, "Native");
            });
            ui.end_row();

            ui.label("Tone response");
            ui.horizontal(|ui| {
                ui.radio_value(&mut tg.tone, Tone::Gamma, "Gamma");
                ui.add_enabled(tg.tone == Tone::Gamma, egui::DragValue::new(&mut tg.gamma).range(1.0..=3.0).speed(0.01).fixed_decimals(2));
                ui.radio_value(&mut tg.tone, Tone::Srgb, "sRGB curve");
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
            ui.horizontal(|ui| {
                ui.radio_value(&mut tg.quality, Quality::Low, "Low");
                ui.radio_value(&mut tg.quality, Quality::Medium, "Medium");
                ui.radio_value(&mut tg.quality, Quality::High, "High");
            });
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Higher quality takes more measurements and longer.").color(t.text_dim));
    }

    fn page_run(&mut self, ui: &mut egui::Ui) {
        let t = Tokens::get(ui.ctx());
        ui.heading("Calibrate & profile");
        ui.add_space(4.0);
        let running = self.job.is_some();
        ui.horizontal(|ui| {
            match self.ready() {
                Ok(()) if !running => {
                    if ui.button(RichText::new("Start").font(t.semibold(13.0))).clicked() {
                        let ctx = ui.ctx().clone();
                        self.start_calibration(&ctx);
                    }
                }
                Ok(()) => {
                    if ui.button("Cancel").clicked() {
                        if let Some((_, job)) = self.job.as_mut() {
                            job.cancel();
                        }
                    }
                }
                Err(why) => {
                    ui.add_enabled(false, egui::Button::new("Start"));
                    ui.label(RichText::new(why).color(t.caution));
                }
            }
            if let Some((ok, text)) = &self.status {
                ui.label(RichText::new(text).color(if *ok { t.good } else { t.bad }));
            }
        });

        if running {
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

        if let Some(outcome) = &self.outcome {
            ui.add_space(10.0);
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

        ui.add_space(10.0);
        ui.label(RichText::new("Log").color(t.text_dim));
        egui::Frame::new().fill(t.field).corner_radius(4).inner_margin(6).show(ui, |ui| {
            ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for line in &self.log {
                    ui.label(RichText::new(line).monospace().size(11.5));
                }
            });
        });
    }

    fn prompt_dialog(&mut self, ctx: &egui::Context) {
        let Some(text) = self.prompt.clone() else { return };
        let t = Tokens::get(ctx);
        // Argyll's prompts end with key instructions that don't apply here
        let message = text
            .replace("Hit Esc or Q to give up, any other key to continue:", "")
            .replace("Hit Esc or Q to give up, any other key to retry:", "")
            .trim()
            .to_string();
        let mut answer = None;
        egui::Modal::new(egui::Id::new("argyll-prompt")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(RichText::new("Waiting for you").font(t.semibold(14.0)).color(t.text));
            ui.add_space(6.0);
            ui.label(if message.is_empty() { "Continue?" } else { message.as_str() });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Continue").font(t.semibold(13.0))).clicked() {
                    answer = Some(' ');
                }
                if ui.button("Give up").clicked() {
                    answer = Some('q');
                }
            });
        });
        if let Some(key) = answer {
            if let Some((_, job)) = self.job.as_mut() {
                job.send_key(key);
            }
            self.prompt = None;
        }
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
            if let Some(err) = &self.inventory_error {
                ui.label(RichText::new(err).color(t.bad));
                ui.add_space(8.0);
            }
            match self.step {
                Step::Display => self.page_display(ui),
                Step::Instrument => self.page_instrument(ui),
                Step::Targets => self.page_targets(ui),
                Step::Run => self.page_run(ui),
            }
        });
        self.preferences(&ctx);
        self.prompt_dialog(&ctx);
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
mod tests {
    use super::output_name;

    #[test]
    fn output_name_from_description() {
        assert_eq!(output_name("Monitor 1, Output eDP-1 (Sharp Corporation eDP-1) at 0, 0, width 2560, height 1600"), "eDP-1");
        assert_eq!(output_name("Something else"), "display");
    }
}
