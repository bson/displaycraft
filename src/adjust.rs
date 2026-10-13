//! dispcal's interactive adjustment of the display's own controls, read
//! from its output. DisplayCraft runs it on its own from the Settings page
//! and leaves with Exit (8), so calibrating stays a separate step.
//!
//! Without `-m`, dispcal shows a menu ("Press 1 .. 7") after the instrument
//! is placed. Choosing an adjustment prints an instruction line and a target
//! (or the starting value when the target is native), then rewrites a
//! "Current ..." reading in place about once a second until space is
//! pressed, when the menu returns. For example:
//!
//! ```text
//! Adjust R,G & B gain to get target x,y. Press space when done.
//!    Target Br 185.77, x 0.3325 , y 0.3476
//! / Current Br 117.34, x 0.3126+, y 0.3427+  DE 10.2  R++ G-  B-
//! ```
//!
//! A '+' or '-' after a value says which way it must move; after a channel,
//! which way to turn its gain or offset, doubled for the one that matters
//! most.

/// The adjustments offered, with dispcal's menu key
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    WhitePoint,
    WhiteLevel,
    BlackLevel,
    BlackPoint,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::WhitePoint, Mode::WhiteLevel, Mode::BlackLevel, Mode::BlackPoint];

    pub fn key(self) -> char {
        match self {
            Mode::BlackLevel => '1',
            Mode::WhitePoint => '2',
            Mode::WhiteLevel => '3',
            Mode::BlackPoint => '4',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::WhitePoint => "White point",
            Mode::WhiteLevel => "White level",
            Mode::BlackLevel => "Black level",
            Mode::BlackPoint => "Black point",
        }
    }

    /// What to turn, in the display's own menus
    pub fn controls(self) -> &'static str {
        match self {
            Mode::WhitePoint => "the red, green and blue gain (or color temperature) controls",
            Mode::WhiteLevel => "the brightness (backlight) control",
            Mode::BlackLevel => "the brightness (offset) control, on displays that have one for black",
            Mode::BlackPoint => "the red, green and blue offset controls",
        }
    }

    /// The instruction line that starts this mode
    fn from_instruction(line: &str) -> Option<Mode> {
        if !line.starts_with("Adjust ") {
            return None;
        }
        Some(if line.contains("R,G & B gain") {
            Mode::WhitePoint
        } else if line.contains("R,G & B offsets") {
            Mode::BlackPoint
        } else if line.contains("Contrast or LCD Brightness") {
            Mode::WhiteLevel
        } else if line.contains("CRT brightness") {
            Mode::BlackLevel
        } else {
            return None;
        })
    }
}

/// Which way something must move
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dir {
    Up,
    Down,
    Hold,
}

impl Dir {
    fn from(c: char) -> Dir {
        match c {
            '+' => Dir::Up,
            '-' => Dir::Down,
            _ => Dir::Hold,
        }
    }
}

/// One live reading
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// Luminance, cd/m²
    pub level: f64,
    /// Chromaticity and which way each must move (color modes only)
    pub xy: Option<((f64, Dir), (f64, Dir))>,
    /// CIEDE2000 from the target (color modes only)
    pub de: Option<f64>,
    /// Red, green and blue: which way to turn each, and whether it's the
    /// one to turn first (color modes only)
    pub rgb: Option<[(Dir, bool); 3]>,
    /// Which way the level must move (level modes only)
    pub level_dir: Option<Dir>,
}

/// Parse "/ Current Br 117.34, x 0.3126+, y 0.3427+  DE 10.2  R++ G-  B- "
/// or "\ Current 117.34  +"
pub fn parse_reading(text: &str) -> Option<Reading> {
    let rest = text.trim_start_matches(['/', '\\', ' ']).strip_prefix("Current ")?;
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.first() != Some(&"Br") {
        // A level: "117.34" then an optional direction
        let level = words.first()?.parse().ok()?;
        let level_dir = words.get(1).and_then(|w| w.chars().next()).map(Dir::from).or(Some(Dir::Hold));
        return Some(Reading { level, xy: None, de: None, rgb: None, level_dir });
    }
    let level = words.get(1)?.trim_end_matches(',').parse().ok()?;
    // "x" "0.3126+," and "y" "0.3427+"
    let coord = |name: &str| -> Option<(f64, Dir)> {
        let i = words.iter().position(|w| *w == name)?;
        let w = words.get(i + 1)?.trim_end_matches(',');
        let (num, dir) = match w.chars().last()? {
            c @ ('+' | '-' | '=') => (&w[..w.len() - 1], Dir::from(c)),
            _ => (w, Dir::Hold),
        };
        Some((num.parse().ok()?, dir))
    };
    let xy = coord("x").zip(coord("y"));
    // "DE 10.2", or "DE 2K  0.4" when the target is native
    let de = words.iter().position(|w| *w == "DE").and_then(|i| {
        let next = if words.get(i + 1) == Some(&"2K") { i + 2 } else { i + 1 };
        words.get(next)?.parse().ok()
    });
    let channel = |c: char| -> Option<(Dir, bool)> {
        let w = words.iter().find(|w| w.starts_with(c) && w.len() >= 2 && w.len() <= 3)?;
        let mut chars = w.chars().skip(1);
        let dir = Dir::from(chars.next()?);
        Some((dir, chars.next().is_some_and(|c| c == '+' || c == '-')))
    };
    let rgb = match (channel('R'), channel('G'), channel('B')) {
        (Some(r), Some(g), Some(b)) => Some([r, g, b]),
        _ => None,
    };
    Some(Reading { level, xy, de, rgb, level_dir: None })
}

/// dispcal's target line for showing: "Target Br 185.77, x 0.3325 , y
/// 0.3476" -> "Target: 185.8 cd/m², x 0.3325, y 0.3476"; "Target 0.25" ->
/// "Target: 0.25 cd/m²"; "Initial 117.34" -> "Started at 117.3 cd/m²".
/// Anything else is returned as it is.
pub fn target_text(line: &str) -> String {
    let (lead, rest) = if let Some(r) = line.strip_prefix("Target") {
        ("Target:", r)
    } else if let Some(r) = line.strip_prefix("Initial") {
        ("Started at", r)
    } else {
        return line.to_string();
    };
    let words: Vec<&str> = rest.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    let num = |i: usize| words.get(i).and_then(|w| w.parse::<f64>().ok());
    let level = |v: f64| if v < 10.0 { format!("{v:.2} cd/m²") } else { format!("{v:.1} cd/m²") };
    match words.first() {
        Some(&"Br") => match (num(1), num(3), num(5)) {
            (Some(b), Some(x), Some(y)) => format!("{lead} {}, x {x:.4}, y {y:.4}", level(b)),
            _ => line.to_string(),
        },
        _ => match num(0) {
            Some(v) => format!("{lead} {}", level(v)),
            None => line.to_string(),
        },
    }
}

/// What dispcal is doing in its adjustment stage
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Waiting at the menu
    pub menu: bool,
    pub mode: Option<Mode>,
    /// "Target Br 185.77, x 0.3325 , y 0.3476" or, for a native target,
    /// "Initial 117.34", without the leading spaces
    pub target: Option<String>,
    pub reading: Option<Reading>,
}

impl State {
    /// Take in a complete line of dispcal's output
    pub fn line(&mut self, line: &str) {
        let line = line.trim();
        if line.starts_with("Press 1 .. 7") {
            *self = State { menu: true, ..State::default() };
        } else if let Some(mode) = Mode::from_instruction(line) {
            *self = State { mode: Some(mode), ..State::default() };
        } else if self.mode.is_some() && (line.starts_with("Target") || line.starts_with("Initial")) {
            self.target = Some(line.to_string());
        } else if line.starts_with("Target white =") || line.starts_with("Setting up") {
            // Before the menu, or calibrating after it
        } else if self.menu && !line.starts_with(char::is_numeric) {
            // Anything but the menu's own entries means it has moved on
            self.menu = false;
        }
    }

    /// Take in a live reading
    pub fn live(&mut self, text: &str) {
        if self.mode.is_some() {
            if let Some(r) = parse_reading(text) {
                self.reading = Some(r);
            }
        }
    }

    pub fn active(&self) -> bool {
        self.menu || self.mode.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_white_point_and_level() {
        let r = parse_reading("/ Current Br 117.34, x 0.3126+, y 0.3427-  DE 10.2  R++ G-  B=  ").unwrap();
        assert_eq!(r.level, 117.34);
        assert_eq!(r.xy, Some(((0.3126, Dir::Up), (0.3427, Dir::Down))));
        assert_eq!(r.de, Some(10.2));
        assert_eq!(r.rgb, Some([(Dir::Up, true), (Dir::Down, false), (Dir::Hold, false)]));
        let native = parse_reading("\\ Current Br 120.00, x 0.3127=, y 0.3290+  CDT 6504K DE 2K  0.4  R+  G=  B- ").unwrap();
        assert_eq!(native.de, Some(0.4));
        let level = parse_reading("\\ Current 117.15   ").unwrap();
        assert_eq!((level.level, level.level_dir, level.xy), (117.15, Some(Dir::Hold), None));
        let level = parse_reading("/ Current 95.20  +").unwrap();
        assert_eq!(level.level_dir, Some(Dir::Up));
        assert!(parse_reading("Target Br 185.77").is_none());
    }

    #[test]
    fn target_lines() {
        assert_eq!(target_text("Target Br 185.77, x 0.3325 , y 0.3476"), "Target: 185.8 cd/m², x 0.3325, y 0.3476");
        assert_eq!(target_text("Target 0.25"), "Target: 0.25 cd/m²");
        assert_eq!(target_text("Initial 117.34"), "Started at 117.3 cd/m²");
        assert_eq!(target_text("Initial Br 120.00, x 0.3127 , y 0.3290 , CDT 6504K DE 2K  0.4"), "Started at 120.0 cd/m², x 0.3127, y 0.3290");
    }

    #[test]
    fn follows_the_menu() {
        let mut s = State::default();
        s.line("Target white = 5500.000000 degrees kelvin Daylight spectrum");
        assert!(!s.active());
        s.line("Press 1 .. 7");
        s.line("1) Black level (CRT: Offset/Brightness)");
        s.line("8) Exit");
        assert!(s.menu);
        s.line("Adjust R,G & B gain to get target x,y. Press space when done.");
        s.line("   Target Br 185.77, x 0.3325 , y 0.3476 ");
        assert_eq!((s.mode, s.target.as_deref()), (Some(Mode::WhitePoint), Some("Target Br 185.77, x 0.3325 , y 0.3476")));
        s.live("/ Current Br 117.34, x 0.3126+, y 0.3427+  DE 10.2  R++ G-  B-  ");
        assert_eq!(s.reading.as_ref().unwrap().level, 117.34);
        s.line("Press 1 .. 7");
        assert!(s.menu && s.mode.is_none() && s.reading.is_none());
        s.line("Adjust CRT Contrast or LCD Brightness to desired level. Press space when done.");
        assert_eq!(s.mode, Some(Mode::WhiteLevel));
        s.line("Press 1 .. 7");
        s.line("Commencing device calibration");
        assert!(!s.active());
    }
}
