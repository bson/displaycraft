//! Running ArgyllCMS tools as separate programs and reading their output.
//!
//! The tools are written for a terminal: they report progress with "\r",
//! ask questions with prompts that have no trailing newline, and read single
//! keys from stdin. We run them with unbuffered output on pipes and turn that
//! into events.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use serde::{Deserialize, Serialize};

/// Where the Argyll tools are: the directory set in preferences if there is
/// one, otherwise the bundled ArgyllCMS, then PATH. A preferred directory
/// is used even if it's wrong, so the problem shows rather than another
/// ArgyllCMS being picked up silently.
pub fn find_bin_dir(preferred: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = preferred {
        return Some(dir.to_path_buf());
    }
    crate::system::current().bundled_dir().or_else(|| {
        std::env::var_os("PATH")
            .and_then(|path| std::env::split_paths(&path).find(|dir| dir.join("dispcal").exists()))
    })
}

/// The two ways the bundled ArgyllCMS can show test patches
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum WindowSystem {
    /// Through KWin, on KDE Plasma's Wayland sessions
    Wayland,
    /// On X11 sessions (GNOME, KDE and others), and through XWayland on
    /// other Wayland desktops
    X11,
}

impl WindowSystem {
    /// As ARGYLL_WINDOW_SYSTEM and dispwin -Q name it
    fn name(self) -> &'static str {
        match self {
            WindowSystem::Wayland => "wayland",
            WindowSystem::X11 => "x11",
        }
    }

    fn from_name(name: &str) -> Option<WindowSystem> {
        match name {
            "wayland" => Some(WindowSystem::Wayland),
            "x11" => Some(WindowSystem::X11),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WindowSystem::Wayland => "KDE + Wayland",
            WindowSystem::X11 => "GNOME, KDE + X11",
        }
    }
}

/// The display system every ArgyllCMS program started from now on is told
/// to use. Global rather than passed along, as every program DisplayCraft
/// runs must agree: displays listed with one can't be measured with the
/// other.
static WINDOW_SYSTEM: Mutex<Option<WindowSystem>> = Mutex::new(None);

/// None leaves the choice to ArgyllCMS: its own detection for the bundled
/// one, and whatever another ArgyllCMS does (which won't know the
/// variable).
pub fn set_window_system(ws: Option<WindowSystem>) {
    *WINDOW_SYSTEM.lock().unwrap() = ws;
}

/// An ArgyllCMS program, told which display system to use
fn command(program: impl AsRef<OsStr>) -> Command {
    told(Command::new(program))
}

/// `cmd`, which runs an ArgyllCMS program, told which display system to use
fn told(mut cmd: Command) -> Command {
    if let Some(ws) = *WINDOW_SYSTEM.lock().unwrap() {
        cmd.env("ARGYLL_WINDOW_SYSTEM", ws.name());
    }
    cmd
}

/// Whether ArgyllCMS can drive the displays in this session, as it reports
/// with `dispwin -Q`.
#[derive(Clone, Debug, PartialEq)]
pub enum Platform {
    /// With a description, e.g. "KDE Plasma on Wayland"
    Supported(String),
    /// It runs, with the limitations given, e.g. through XWayland
    Limited(String),
    /// With what's missing
    Unsupported(String),
    /// This ArgyllCMS doesn't know -Q, so it isn't the one DisplayCraft
    /// comes with
    Unknown,
}

/// The session as ArgyllCMS sees it, and what it makes of it. The bundled
/// ArgyllCMS fills in everything; one that doesn't know -Q, nothing but
/// `platform`.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// $XDG_CURRENT_DESKTOP, e.g. "KDE" or "GNOME"
    pub desktop: Option<String>,
    /// e.g. "KDE" (ArgyllCMS's "KWin") or "GNOME Shell", or "none" outside Wayland
    pub compositor: Option<String>,
    /// e.g. "Xorg :0" or "XWayland :0", or "none"
    pub x11_server: Option<String>,
    pub x11_window_manager: Option<String>,
    /// What ArgyllCMS would choose by itself
    pub automatic: Option<WindowSystem>,
    /// What it's using, which differs when told
    pub display_system: Option<WindowSystem>,
    pub platform: Platform,
}

impl Session {
    /// e.g. "KDE on Wayland, with XWayland :0", "GNOME on X11 (Xorg :0,
    /// window manager GNOME Shell)" or "Wayland (Sway)"
    pub fn describe(&self) -> String {
        let known = |v: &Option<String>| v.clone().filter(|v| v != "none" && v != "unknown");
        let desktop = known(&self.desktop);
        let text = match (known(&self.compositor), known(&self.x11_server)) {
            (Some(c), x) => {
                // Named once when the compositor is the desktop's own
                let mut t = match &desktop {
                    Some(d) if c.contains(d.as_str()) || d.contains(c.as_str()) => format!("{d} on Wayland"),
                    Some(d) => format!("{d} on Wayland ({c})"),
                    None => format!("Wayland ({c})"),
                };
                if let Some(x) = x {
                    t += &format!(", with {x}");
                }
                t
            }
            (None, Some(x)) => {
                let wm = known(&self.x11_window_manager).map(|w| format!(", window manager {w}")).unwrap_or_default();
                match &desktop {
                    Some(d) => format!("{d} on X11 ({x}{wm})"),
                    None => format!("X11 ({x}{wm})"),
                }
            }
            (None, None) => desktop.unwrap_or_else(|| "unknown".into()),
        };
        text
    }
}

/// ArgyllCMS's wording for what DisplayCraft shows: KWin is called KDE
fn as_shown(text: &str) -> String {
    text.replace("KDE Plasma (KWin)", "KDE Plasma").replace("KDE Plasma's KWin", "KDE Plasma").replace("KWin", "KDE")
}

/// The profile the system associates with display `display` (the `-d`
/// value), as `dispwin -W` reports it: Some(None) if there's none, None if
/// it couldn't say (e.g. an ArgyllCMS without -W).
pub fn installed_profile(bin: &Path, display: &str) -> Option<Option<PathBuf>> {
    let out = command(bin.join("dispwin")).args(["-d", display, "-W"]).stdin(Stdio::null()).output().ok()?;
    parse_installed_profile(&String::from_utf8_lossy(&out.stdout))
}

fn parse_installed_profile(text: &str) -> Option<Option<PathBuf>> {
    let value = text.lines().find_map(|l| l.strip_prefix("profile: "))?.trim();
    // On X11 it can be the root window's atom name rather than a file
    Some((value != "none").then(|| PathBuf::from(value)).filter(|p| p.is_absolute()))
}

/// Ask the ArgyllCMS in `bin` about this session, with the display system
/// set by set_window_system()
pub fn check_platform(bin: &Path) -> Session {
    run_check(command(bin.join("dispwin")))
}

/// The same for display system `ws` whatever set_window_system() said:
/// None lets ArgyllCMS choose
pub fn check_platform_as(bin: &Path, ws: Option<WindowSystem>) -> Session {
    let mut cmd = Command::new(bin.join("dispwin"));
    match ws {
        Some(ws) => cmd.env("ARGYLL_WINDOW_SYSTEM", ws.name()),
        None => cmd.env_remove("ARGYLL_WINDOW_SYSTEM"),
    };
    run_check(cmd)
}

fn run_check(mut cmd: Command) -> Session {
    match cmd.arg("-Q").stdin(Stdio::null()).output() {
        Ok(out) => parse_session(&String::from_utf8_lossy(&out.stdout)),
        Err(e) => Session {
            desktop: None,
            compositor: None,
            x11_server: None,
            x11_window_manager: None,
            automatic: None,
            display_system: None,
            platform: Platform::Unsupported(format!("dispwin won't run: {e}")),
        },
    }
}

fn parse_session(text: &str) -> Session {
    let mut s = Session {
        desktop: None,
        compositor: None,
        x11_server: None,
        x11_window_manager: None,
        automatic: None,
        display_system: None,
        platform: Platform::Unknown,
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once(": ") else { continue };
        let value = as_shown(value.trim());
        match key {
            "desktop" => s.desktop = Some(value),
            "wayland compositor" => s.compositor = Some(value),
            "x11 server" => s.x11_server = Some(value),
            "x11 window manager" => s.x11_window_manager = Some(value),
            "automatic display system" => s.automatic = WindowSystem::from_name(&value),
            // "x11 (ARGYLL_WINDOW_SYSTEM)" when told
            "display system" => {
                s.display_system = value.split_whitespace().next().and_then(WindowSystem::from_name)
            }
            "platform supported" => s.platform = Platform::Supported(value),
            "platform limited" => s.platform = Platform::Limited(value),
            "platform unsupported" => s.platform = Platform::Unsupported(value),
            _ => {}
        }
    }
    s
}

/// Check that `dir` holds a working ArgyllCMS. Returns its version, or why not.
pub fn probe(dir: &Path) -> Result<String, String> {
    if !dir.is_dir() {
        return Err("Not a directory".into());
    }
    if !dir.join("dispcal").is_file() {
        return Err("No ArgyllCMS programs (dispcal) in this directory".into());
    }
    match inventory(dir) {
        Ok(inv) if !inv.version.is_empty() => Ok(inv.version),
        Ok(_) => Err("dispcal here didn't report a version".into()),
        Err(e) => Err(format!("dispcal here won't run: {e}")),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// What to pass to the option, e.g. "1" for "-d 1" or "l" for "-y l".
    pub value: String,
    pub description: String,
}

/// What dispcal can drive on this system.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub version: String,
    pub displays: Vec<Choice>,
    /// Only ports Argyll identified as instruments. It also lists every
    /// serial port, whether or not anything is attached.
    pub instruments: Vec<Choice>,
    /// Display types for the default instrument (dispcal's -y).
    pub display_types: Vec<Choice>,
}

/// Run `dispcal -?` and parse the lists from its usage text.
pub fn inventory(bin: &Path) -> std::io::Result<Inventory> {
    let out = command(bin.join("dispcal")).arg("-?").stdin(Stdio::null()).output()?;
    // Usage goes to stderr, with a non-zero exit status
    let text = String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
    Ok(parse_inventory(&text))
}

fn parse_inventory(text: &str) -> Inventory {
    #[derive(PartialEq)]
    enum Section {
        None,
        Displays,
        Instruments,
        DisplayTypes,
    }
    let mut inv = Inventory::default();
    let mut section = Section::None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Calibrate a Display, Version ") {
            inv.version = rest.trim().to_string();
            continue;
        }
        if line.starts_with(" -") {
            // " -d n[,m]" from every current Linux build, X11 support or
            // not; " -d n" from older Wayland-only builds of the fork and
            // from Windows and macOS builds, which "Other" may point at
            section = if line.starts_with(" -d n[,m] ") || line.starts_with(" -d n ") {
                Section::Displays
            } else if line.starts_with(" -c listno") {
                Section::Instruments
            } else if let Some(rest) = line.strip_prefix(" -y ") {
                // The first entry is on the option line itself, e.g.
                // " -y n   i1D3: Non-Refresh display [Default,CB1]"
                push_display_type(&mut inv.display_types, rest);
                Section::DisplayTypes
            } else {
                Section::None
            };
            continue;
        }
        match section {
            Section::Displays | Section::Instruments => {
                // "    1 = 'description'"
                if let Some((num, desc)) = line.trim().split_once(" = ") {
                    if num.chars().all(|c| c.is_ascii_digit()) {
                        let desc = desc.trim().trim_matches('\'').to_string();
                        let choice = Choice { value: num.to_string(), description: desc };
                        if section == Section::Displays {
                            inv.displays.push(choice);
                        } else if choice.description.contains('(') {
                            inv.instruments.push(choice);
                        }
                    }
                }
            }
            Section::DisplayTypes => push_display_type(&mut inv.display_types, line),
            Section::None => {}
        }
    }
    inv
}

/// Add the display type(s) from one line of dispcal's -y list:
/// "    r|c                i1D3: Refresh display [CB2]", or the generic
/// "    l|c                Other: l = LCD, c = CRT".
fn push_display_type(types: &mut Vec<Choice>, line: &str) {
    let trimmed = line.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let (Some(value), Some(desc)) = (parts.next(), parts.next()) else { return };
    let desc = desc.trim();
    if let Some(other) = desc.strip_prefix("Other:") {
        for item in other.split(',') {
            if let Some((v, d)) = item.split_once('=') {
                let v = v.trim();
                if !types.iter().any(|t| t.value == v) {
                    types.push(Choice { value: v.to_string(), description: d.trim().to_string() });
                }
            }
        }
        return;
    }
    // Alternatives like "r|c" select the same type; pass the first
    let value = value.split('|').next().unwrap_or(value);
    if value.is_empty() || value.len() > 2 || desc.is_empty() {
        return;
    }
    // The list is already for the selected instrument, so drop its name
    let desc = match desc.split_once(": ") {
        Some((inst, rest)) if !inst.contains(' ') => rest,
        _ => desc,
    };
    types.push(Choice { value: value.to_string(), description: desc.to_string() });
}

#[derive(Debug)]
pub enum Event {
    /// A complete line of output.
    Line(String),
    /// "patch N of M".
    Progress(u32, u32),
    /// The tool waits for a key, e.g. to place the instrument or retry a
    /// failed reading. `before` is the last few lines printed before it,
    /// which say what went wrong, if anything; they come as Lines too.
    Prompt { prompt: String, before: Vec<String> },
    /// A reading dispcal rewrites in place during interactive adjustment
    /// ("/ Current Br 117.34, ..."). Not logged: there's one a second.
    Live(String),
}

/// A running Argyll tool.
pub struct Job {
    child: Child,
    stdin: Option<ChildStdin>,
    pub events: Receiver<Event>,
}

impl Job {
    /// Start `tool` from `bin` with `args`. `wake` is called after each
    /// event, so the UI can repaint.
    pub fn start(bin: &Path, tool: &str, args: &[String], wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Job> {
        // So progress and prompts arrive as they're printed rather than
        // when a pipe buffer fills (see System::live_output)
        let mut child = told(crate::system::current().live_output(&bin.join(tool)))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let (tx, rx) = mpsc::channel();
        let wake = std::sync::Arc::new(wake);
        for stream in [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let tx = tx.clone();
            let wake = wake.clone();
            thread::spawn(move || read_output(stream, tx, move || wake()));
        }
        Ok(Job { child, stdin, events: rx })
    }

    /// Answer a prompt: any key continues, 'q' gives up.
    pub fn send_key(&mut self, key: char) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(key.to_string().as_bytes());
            let _ = stdin.flush();
        }
    }

    /// Exit status once the tool has finished, without blocking.
    pub fn try_wait(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            Ok(None) => None,
            Err(_) => Some(None),
        }
    }

    /// Stop the tool. SIGTERM lets dispwin's signal handler restore the
    /// display's color settings, which SIGKILL wouldn't.
    pub fn cancel(&mut self) {
        let _ = Command::new("kill").arg("-TERM").arg(self.child.id().to_string()).status();
    }
}

/// A job dropped while its tool still runs (DisplayCraft quitting or
/// panicking mid-measurement) is cancelled. Otherwise the tool only sees its
/// input close, and dispcal then exits without restoring the display: KWin
/// was left in sRGB mode with its accuracy preference changed.
impl Drop for Job {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.cancel();
        }
    }
}

/// Noise from Argyll finding that stdin isn't a terminal
fn is_noise(line: &str) -> bool {
    line.contains("_con_char: tc") && line.contains("on stdin")
}

fn read_output(mut stream: Box<dyn Read + Send>, tx: Sender<Event>, wake: impl Fn()) {
    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    let mut prompted = false;
    let mut before: Vec<String> = Vec::new();
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pending.push_str(&String::from_utf8_lossy(&buf[..n]));
        // Both "\r" (progress) and "\n" end a line
        while let Some(pos) = pending.find(['\r', '\n']) {
            let line: String = pending.drain(..=pos).collect();
            let line = clean(&line);
            if line.is_empty() {
                continue;
            }
            if let Some(progress) = parse_progress(&line) {
                let _ = tx.send(progress);
            } else if !prompted && is_prompt(&line) {
                // Some prompts end with a newline, e.g. after a failed
                // instrument calibration
                let _ = tx.send(Event::Prompt { prompt: line, before: std::mem::take(&mut before) });
                prompted = true;
                continue;
            } else {
                before.push(line.clone());
                if before.len() > 4 {
                    before.remove(0);
                }
                let _ = tx.send(Event::Line(line));
            }
            prompted = false;
        }
        // A prompt has no newline, so look at what's left over
        let rest = clean(&pending);
        // A live reading is ended only by the "\r" that starts the next
        // one, so it's passed on now and dropped
        if rest.trim_start_matches(['/', '\\', ' ']).starts_with("Current ") {
            let _ = tx.send(Event::Live(rest));
            pending.clear();
            wake();
            continue;
        }
        if !prompted && is_prompt(&rest) {
            let _ = tx.send(Event::Prompt { prompt: rest, before: std::mem::take(&mut before) });
            pending.clear();
            prompted = true;
        }
        wake();
    }
    let rest = clean(&pending);
    if !rest.is_empty() {
        let _ = tx.send(Event::Line(rest));
    }
    wake();
}

/// Whether ArgyllCMS is waiting for a key. Its prompts are worded several
/// ways ("Hit Esc or Q to give up, any other key to retry:", "Hit any key to
/// retry, or Esc or Q to abort:", "... or hit Esc or Q to abort: "), and
/// some have no "any other key".
fn is_prompt(text: &str) -> bool {
    text.to_ascii_lowercase().contains("esc or q") || text.contains("any other key") || text.ends_with("continue:")
}

fn clean(s: &str) -> String {
    // The noise is printed without line breaks, so cut it out of the middle
    let mut s = s.to_string();
    while let Some(start) = s.find("next_con_char: tc").or_else(|| s.find("poll_con_char: tc")) {
        match s[start..].find("on stdin") {
            Some(end) => s.replace_range(start..start + end + "on stdin".len(), ""),
            None => break,
        }
    }
    let s = s.trim().to_string();
    if is_noise(&s) { String::new() } else { s }
}

fn parse_progress(line: &str) -> Option<Event> {
    // "patch 12 of 48"
    let rest = line.strip_prefix("patch ")?;
    let (n, m) = rest.split_once(" of ")?;
    let m: String = m.chars().take_while(|c| c.is_ascii_digit()).collect();
    Some(Event::Progress(n.trim().parse().ok()?, m.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dispcal_usage() {
        // Raw string: Argyll's indentation is what the parser keys on
        let text = r#"Calibrate a Display, Version 3.5.0
 -d n[,m]             Choose the display n from the following list (default 1)
                      Optionally choose different display m for VideoLUT access (X11)
    1 = 'Monitor 1, Output eDP-1 (Sharp Corporation eDP-1) at 0, 0, width 2560, height 1600'
 -d dummy             Dummy (non-existant, invisible) display
 -c listno            Choose instrument from the following list (default 1)
    1 = '/dev/bus/usb/001/015 (X-Rite i1 DisplayPro, ColorMunki Display)'
    2 = '/dev/ttyS0'
 -m                   Skip adjustment of the monitor controls
 -y l|c                Other: l = LCD, c = CRT
 -t [temp]            White Daylight locus target
"#;
        let inv = parse_inventory(text);
        assert_eq!(inv.version, "3.5.0");
        assert_eq!(inv.displays.len(), 1);
        assert_eq!(inv.displays[0].value, "1");
        assert!(inv.displays[0].description.starts_with("Monitor 1, Output eDP-1"));
        assert_eq!(inv.instruments.len(), 1, "bare serial ports are dropped");
        assert_eq!(inv.display_types, vec![
            Choice { value: "l".into(), description: "LCD".into() },
            Choice { value: "c".into(), description: "CRT".into() },
        ]);
    }

    #[test]
    fn parses_instrument_display_types() {
        let text = r#"Calibrate a Display, Version 3.5.0
 -y n                  i1D3: Non-Refresh display [Default,CB1]
    r|c                i1D3: Refresh display [CB2]
    l                  i1D3: LCD CCFL IPS (CCFL AC EIZO HP with CORRECTION)
    e                  i1D3: LCD White LED IPS (WLED AC LG Samsung)
    l|c                Other: l = LCD, c = CRT
 -t [temp]            White Daylight locus target
"#;
        let inv = parse_inventory(text);
        let values: Vec<&str> = inv.display_types.iter().map(|t| t.value.as_str()).collect();
        assert_eq!(values, ["n", "r", "l", "e", "c"], "first entry kept, alternatives reduced, generic l not duplicated");
        assert_eq!(inv.display_types[0].description, "Non-Refresh display [Default,CB1]");
        assert_eq!(inv.display_types[3].description, "LCD White LED IPS (WLED AC LG Samsung)");
        assert_eq!(inv.display_types[4].description, "CRT");
    }

    #[test]
    fn parses_display_option_without_x11() {
        let text = r#" -d n                 Choose the display from the following list (default 1)
    1 = 'Monitor 1, Output eDP-1 (Sharp Corporation eDP-1) at 0, 0, width 2560, height 1600'
 -dweb[:port]         Display via a web server at port (default 8080)
"#;
        assert_eq!(parse_inventory(text).displays.len(), 1);
    }

    #[test]
    fn parses_installed_profile() {
        assert_eq!(parse_installed_profile("profile: /Users/b/Library/ColorSync/Profiles/x.icc\n"),
                   Some(Some(PathBuf::from("/Users/b/Library/ColorSync/Profiles/x.icc"))));
        assert_eq!(parse_installed_profile("profile: none\n"), Some(None));
        assert_eq!(parse_installed_profile("profile: _ICC_PROFILE\n"), Some(None));
        assert_eq!(parse_installed_profile("usage: dispwin [options]\n"), None);
    }

    #[test]
    fn parses_platform_report() {
        let s = parse_session(
            "desktop: KDE\nwayland compositor: KWin\nx11 server: XWayland :0\nx11 window manager: KWin\n\
             automatic display system: wayland\ndisplay system: x11 (ARGYLL_WINDOW_SYSTEM)\n\
             platform limited: X11 through XWayland :0 on KWin: the compositor may alter the test patch colors\n",
        );
        assert_eq!(s.desktop.as_deref(), Some("KDE"));
        assert_eq!((s.automatic, s.display_system), (Some(WindowSystem::Wayland), Some(WindowSystem::X11)));
        assert_eq!(s.platform, Platform::Limited("X11 through XWayland :0 on KDE: the compositor may alter the test patch colors".into()));
        assert_eq!(s.describe(), "KDE on Wayland, with XWayland :0");
        let x = parse_session("desktop: GNOME\nwayland compositor: none\nx11 server: Xorg :0\nx11 window manager: GNOME Shell\n");
        assert_eq!(x.describe(), "GNOME on X11 (Xorg :0, window manager GNOME Shell)");
        assert_eq!(parse_session("platform supported: KDE Plasma (KWin) on Wayland\n").platform,
                   Platform::Supported("KDE Plasma on Wayland".into()));
        let lacks = parse_session("platform unsupported: the Wayland compositor lacks x, which only KDE Plasma's KWin provides together\n");
        assert_eq!(lacks.platform, Platform::Unsupported("the Wayland compositor lacks x, which only KDE Plasma provides together".into()));
        assert_eq!(parse_session("platform unsupported: can't connect to a Wayland compositor\n").platform,
                   Platform::Unsupported("can't connect to a Wayland compositor".into()));
        // An older dispwin prints its usage for the unknown option
        let old = parse_session("Test display patch window, Version 3.4.1\nusage: dispwin\n");
        assert_eq!((old.platform, old.automatic), (Platform::Unknown, None));
    }

    #[test]
    fn parses_progress_and_strips_noise() {
        assert!(matches!(parse_progress("patch 12 of 48"), Some(Event::Progress(12, 48))));
        let noisy = "patch 3 of 9poll_con_char: tcgetattr failed with 'Inappropriate ioctl for device' on stdin";
        assert_eq!(clean(noisy), "patch 3 of 9");
    }
}
