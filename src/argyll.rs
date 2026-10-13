//! Running ArgyllCMS tools as separate programs and reading their output.
//!
//! The tools are written for a terminal: they report progress with "\r",
//! ask questions with prompts that have no trailing newline, and read single
//! keys from stdin. We run them with unbuffered output on pipes and turn that
//! into events.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// Where the Argyll tools are: the directory set in preferences if there is
/// one, otherwise $DISPLAYCRAFT_ARGYLL_BIN, the Wayland build in /opt, then
/// PATH. A preferred directory is used even if it's wrong, so the problem
/// shows rather than another ArgyllCMS being picked up silently.
pub fn find_bin_dir(preferred: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = preferred {
        return Some(dir.to_path_buf());
    }
    auto_bin_dir()
}

/// The directory automatic lookup finds, ignoring preferences.
pub fn auto_bin_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DISPLAYCRAFT_ARGYLL_BIN") {
        return Some(PathBuf::from(dir));
    }
    let wayland = PathBuf::from("/opt/argyll-kde-wayland/bin");
    if wayland.join("dispcal").exists() {
        return Some(wayland);
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find(|dir| dir.join("dispcal").exists())
    })
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
    let out = Command::new(bin.join("dispcal")).arg("-?").stdin(Stdio::null()).output()?;
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
            section = if line.starts_with(" -d n ") {
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
    /// The tool waits for a key, e.g. to place the instrument. The text is
    /// the prompt's recent context.
    Prompt(String),
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
        // stdbuf, so progress and prompts arrive as they're printed
        // rather than when a pipe buffer fills.
        let mut child = Command::new("stdbuf")
            .arg("-o0")
            .arg("-e0")
            .arg(bin.join(tool))
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

/// Noise from Argyll finding that stdin isn't a terminal
fn is_noise(line: &str) -> bool {
    line.contains("_con_char: tc") && line.contains("on stdin")
}

fn read_output(mut stream: Box<dyn Read + Send>, tx: Sender<Event>, wake: impl Fn()) {
    let mut buf = [0u8; 4096];
    let mut pending = String::new();
    let mut prompted = false;
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
            } else {
                let _ = tx.send(Event::Line(line));
            }
            prompted = false;
        }
        // A prompt has no newline, so look at what's left over
        let rest = clean(&pending);
        if !prompted && (rest.contains("any other key") || rest.ends_with("continue:")) {
            let _ = tx.send(Event::Prompt(rest));
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
 -d n                 Choose the display from the following list (default 1)
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
    fn parses_progress_and_strips_noise() {
        assert!(matches!(parse_progress("patch 12 of 48"), Some(Event::Progress(12, 48))));
        let noisy = "patch 3 of 9poll_con_char: tcgetattr failed with 'Inappropriate ioctl for device' on stdin";
        assert_eq!(clean(noisy), "patch 3 of 9");
    }
}
