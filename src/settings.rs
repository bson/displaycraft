//! Preferences kept between runs, in settings.toml in the system's config
//! folder for DisplayCraft (System::config_dir).

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::argyll::WindowSystem;
use crate::targets::Targets;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Directory holding an ArgyllCMS other than the bundled one. None
    /// means use the bundled one.
    pub argyll_bin: Option<PathBuf>,
    /// The bundled ArgyllCMS's display system. None means as detected.
    pub argyll_variant: Option<WindowSystem>,
    /// Theme label, e.g. "Charcoal".
    pub theme: Option<String>,
    /// Colorimeter correction (.ccss/.ccmx) passed with -X. None means use
    /// the display type instead.
    pub correction: Option<PathBuf>,
    /// The last calibration targets chosen
    pub targets: Targets,
    /// Calibrate to match `reference_file`, whose values are in `targets`
    pub match_reference: bool,
    pub reference_file: Option<PathBuf>,
    /// The targets from before matching, put back when matching is turned
    /// off
    pub unmatched_targets: Option<Targets>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            argyll_bin: None,
            argyll_variant: None,
            theme: None,
            correction: None,
            targets: Targets::default(),
            match_reference: false,
            reference_file: None,
            unmatched_targets: None,
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(crate::system::current().config_dir()?.join("settings.toml"))
}

impl Settings {
    /// Missing or unreadable settings give the defaults, so a damaged file
    /// never keeps the app from starting.
    pub fn load() -> Settings {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Written to a temporary file and renamed into place, so a crash
    /// mid-write leaves the previous settings intact.
    pub fn save(&self) -> std::io::Result<()> {
        let path = path().ok_or_else(|| std::io::Error::other("no HOME"))?;
        let dir = path.parent().expect("settings path has a directory");
        std::fs::create_dir_all(dir)?;
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_unknown_or_missing_fields() {
        let s = Settings {
            argyll_bin: Some("/opt/argyll-kde-wayland/bin".into()),
            argyll_variant: Some(WindowSystem::X11),
            theme: Some("Paper".into()),
            correction: Some("/usr/local/share/ArgyllCMS/WLEDFamily_07Feb11.ccss".into()),
            targets: Targets { white: crate::targets::White::D55, ..Targets::default() },
            match_reference: true,
            reference_file: Some("/home/u/.local/share/displaycraft/DP-3 2026-10-14.dcref".into()),
            unmatched_targets: Some(Targets::default()),
        };
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
        let older: Settings = toml::from_str("theme = \"Midnight\"\nsomething_new = 3\n").unwrap();
        assert_eq!(older.theme.as_deref(), Some("Midnight"));
        assert_eq!(older.argyll_bin, None);
    }
}
