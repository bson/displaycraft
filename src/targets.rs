//! Calibration targets: white point, tonal response, brightness and quality.
//! Saved with the settings, so they survive restarts.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum White {
    D50,
    D55,
    D65,
    /// A daylight color temperature of the user's choosing
    Custom,
    Native,
    /// Exact CIE xy chromaticity, from a reference display: a temperature
    /// alone would lose any tint off the daylight line
    Chromaticity,
}

impl White {
    /// The common choices, each a daylight temperature
    pub const PRESETS: [(White, &'static str, f64); 3] =
        [(White::D50, "D50", 5000.0), (White::D55, "D55", 5500.0), (White::D65, "D65", 6500.0)];
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Tone {
    Gamma22,
    Gamma18,
    /// A gamma of the user's choosing
    Gamma,
    Srgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Quality {
    Low,
    Medium,
    High,
}

/// Whether a LUT profile may be made at Low quality. Its 100 patches are
/// too few: a test LUT verified at average 0.96 / maximum 2.95 where a
/// shaper+matrix from the same patches gave 0.39 / 1.32. Off unless built
/// with the `low-quality-lut` feature.
pub const LOW_QUALITY_LUT: bool = cfg!(feature = "low-quality-lut");

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProfileType {
    /// Per-channel curves and a matrix: smooth, small, understood by every
    /// application
    ShaperMatrix,
    /// A table of measured colors (with matrix tags as a fallback): follows
    /// a panel's quirks more closely, given enough patches
    Lut,
}

impl ProfileType {
    /// colprof's -a value
    pub fn arg(self) -> &'static str {
        match self {
            ProfileType::ShaperMatrix => "s",
            ProfileType::Lut => "X",
        }
    }
}

impl Quality {
    /// How many patches to measure for the profile
    pub fn patches(self) -> u32 {
        match self {
            Quality::Low => 100,
            Quality::Medium => 250,
            Quality::High => 600,
        }
    }

    pub fn arg(self) -> &'static str {
        match self {
            Quality::Low => "l",
            Quality::Medium => "m",
            Quality::High => "h",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Targets {
    pub white: White,
    /// For White::Custom
    pub kelvin: f64,
    /// For White::Chromaticity, CIE x and y. Kept when another white is
    /// chosen, so it can be chosen again.
    pub white_xy: Option<[f64; 2]>,
    pub tone: Tone,
    /// For Tone::Gamma
    pub gamma: f64,
    pub set_brightness: bool,
    pub brightness: f64,
    /// Raise black to `black` cd/m², rather than the device's own. For
    /// matching print's contrast, which device black far exceeds.
    pub set_black: bool,
    pub black: f64,
    pub quality: Quality,
    pub profile: ProfileType,
}

impl Default for Targets {
    fn default() -> Self {
        Targets {
            white: White::D65,
            kelvin: 6500.0,
            white_xy: None,
            tone: Tone::Gamma22,
            gamma: 2.2,
            set_brightness: false,
            brightness: 120.0,
            set_black: false,
            // About a paper's contrast against 120 cd/m² white
            black: 0.5,
            quality: Quality::Medium,
            profile: ProfileType::ShaperMatrix,
        }
    }
}

impl Targets {
    /// Whether this quality and profile type may be combined
    pub fn allows(quality: Quality, profile: ProfileType) -> bool {
        LOW_QUALITY_LUT || !(quality == Quality::Low && profile == ProfileType::Lut)
    }

    /// Make a disallowed combination (e.g. from older settings) allowed, by
    /// raising the quality. Returns true if anything changed.
    pub fn settle(&mut self) -> bool {
        if Self::allows(self.quality, self.profile) {
            return false;
        }
        self.quality = Quality::Medium;
        true
    }

    /// The daylight temperature to calibrate to, or None for native white
    pub fn kelvin(&self) -> Option<f64> {
        match self.white {
            White::Native | White::Chromaticity => None,
            White::Custom => Some(self.kelvin),
            w => White::PRESETS.iter().find(|p| p.0 == w).map(|p| p.2),
        }
    }

    /// dispcal's white point arguments: -t for a daylight temperature, -w
    /// for a chromaticity, none for native
    pub fn white_args(&self) -> Vec<String> {
        match (self.white, self.white_xy) {
            (White::Chromaticity, Some([x, y])) => vec!["-w".into(), format!("{x:.4},{y:.4}")],
            // Chromaticity without one (hand-edited settings) is native
            (White::Chromaticity, None) => Vec::new(),
            _ => self.kelvin().map(|k| vec!["-t".into(), format!("{k:.0}")]).unwrap_or_default(),
        }
    }

    /// dispcal's black arguments: -B for a black level, with -k 1 so the
    /// raised black and the grays near it take white's hue. dispcal's
    /// automatic -k corrects little on panels with a poor black, which a
    /// raised black has room to correct fully.
    pub fn black_args(&self) -> Vec<String> {
        if self.set_black { vec!["-B".into(), format!("{:.3}", self.black), "-k".into(), "1".into()] } else { Vec::new() }
    }

    /// dispcal's -g value
    pub fn gamma_arg(&self) -> String {
        match self.tone {
            Tone::Gamma22 => "2.2".into(),
            Tone::Gamma18 => "1.8".into(),
            Tone::Gamma => format!("{:.2}", self.gamma),
            Tone::Srgb => "s".into(),
        }
    }

    /// e.g. "White D55 (5500 K), gamma 2.2, native brightness, device black,
    /// medium quality, shaper+matrix profile"
    pub fn describe(&self) -> String {
        let white = match (self.white, self.white_xy) {
            (White::Chromaticity, Some([x, y])) => {
                format!("white x {x:.4}, y {y:.4} (about {:.0} K)", crate::report::cct((x, y)))
            }
            (White::Native | White::Chromaticity, _) => "native white".to_string(),
            (White::Custom, _) => format!("white {:.0} K", self.kelvin),
            (w, _) => White::PRESETS.iter().find(|p| p.0 == w).map(|p| format!("white {} ({:.0} K)", p.1, p.2)).unwrap_or_default(),
        };
        let tone = match self.tone {
            Tone::Srgb => "sRGB tone curve".into(),
            _ => format!("gamma {}", self.gamma_arg()),
        };
        let brightness =
            if self.set_brightness { format!("{:.0} cd/m²", self.brightness) } else { "native brightness".into() };
        let black = if self.set_black { format!("black {:.2} cd/m²", self.black) } else { "device black".into() };
        let quality = match self.quality {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
        };
        let profile = match self.profile {
            ProfileType::ShaperMatrix => "shaper+matrix profile",
            ProfileType::Lut => "LUT profile",
        };
        let mut text = format!("{white}, {tone}, {brightness}, {black}, {quality} quality, {profile}");
        text[..1].make_ascii_uppercase();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_description() {
        let mut t = Targets { white: White::D55, ..Targets::default() };
        assert_eq!(t.kelvin(), Some(5500.0));
        assert_eq!(t.gamma_arg(), "2.2");
        assert_eq!(t.describe(), "White D55 (5500 K), gamma 2.2, native brightness, device black, medium quality, shaper+matrix profile");
        assert!(t.black_args().is_empty());
        t.set_black = true;
        assert_eq!(t.black_args(), ["-B", "0.500", "-k", "1"]);
        assert!(t.describe().contains(", black 0.50 cd/m², "), "{}", t.describe());
        t.white = White::Native;
        t.tone = Tone::Gamma;
        t.gamma = 2.4;
        assert_eq!(t.kelvin(), None);
        assert_eq!(t.gamma_arg(), "2.40");
        assert!(t.white_args().is_empty());
        t.white = White::Chromaticity;
        assert!(t.white_args().is_empty());
        t.white_xy = Some([0.3127, 0.329]);
        assert_eq!(t.white_args(), ["-w", "0.3127,0.3290"]);
        assert!(t.describe().starts_with("White x 0.3127, y 0.3290 (about 650"), "{}", t.describe());
        t.white = White::D50;
        assert_eq!(t.white_args(), ["-t", "5000"]);
    }

    #[test]
    fn low_quality_lut() {
        let mut t = Targets { quality: Quality::Low, profile: ProfileType::Lut, ..Targets::default() };
        assert_eq!(t.settle(), !LOW_QUALITY_LUT);
        assert_eq!(t.quality, if LOW_QUALITY_LUT { Quality::Low } else { Quality::Medium });
        assert!(Targets::allows(Quality::Low, ProfileType::ShaperMatrix));
    }
}
