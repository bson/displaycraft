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

impl Quality {
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
    pub tone: Tone,
    /// For Tone::Gamma
    pub gamma: f64,
    pub set_brightness: bool,
    pub brightness: f64,
    pub quality: Quality,
}

impl Default for Targets {
    fn default() -> Self {
        Targets {
            white: White::D65,
            kelvin: 6500.0,
            tone: Tone::Gamma22,
            gamma: 2.2,
            set_brightness: false,
            brightness: 120.0,
            quality: Quality::Medium,
        }
    }
}

impl Targets {
    /// The daylight temperature to calibrate to, or None for native white
    pub fn kelvin(&self) -> Option<f64> {
        match self.white {
            White::Native => None,
            White::Custom => Some(self.kelvin),
            w => White::PRESETS.iter().find(|p| p.0 == w).map(|p| p.2),
        }
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

    /// e.g. "White D55 (5500 K), gamma 2.2, native brightness, medium quality"
    pub fn describe(&self) -> String {
        let white = match self.white {
            White::Native => "native white".to_string(),
            White::Custom => format!("white {:.0} K", self.kelvin),
            w => White::PRESETS.iter().find(|p| p.0 == w).map(|p| format!("white {} ({:.0} K)", p.1, p.2)).unwrap_or_default(),
        };
        let tone = match self.tone {
            Tone::Srgb => "sRGB tone curve".into(),
            _ => format!("gamma {}", self.gamma_arg()),
        };
        let brightness =
            if self.set_brightness { format!("{:.0} cd/m²", self.brightness) } else { "native brightness".into() };
        let quality = match self.quality {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
        };
        let mut text = format!("{white}, {tone}, {brightness}, {quality} quality");
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
        assert_eq!(t.describe(), "White D55 (5500 K), gamma 2.2, native brightness, medium quality");
        t.white = White::Native;
        t.tone = Tone::Gamma;
        t.gamma = 2.4;
        assert_eq!(t.kelvin(), None);
        assert_eq!(t.gamma_arg(), "2.40");
    }
}
