//! Preferences kept between runs, in $XDG_CONFIG_HOME/displaycraft/settings.toml.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Directory holding the ArgyllCMS programs. None means find it
    /// automatically.
    pub argyll_bin: Option<PathBuf>,
    /// Theme label, e.g. "Charcoal".
    pub theme: Option<String>,
}

fn path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("displaycraft").join("settings.toml"))
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
        let s = Settings { argyll_bin: Some("/opt/argyll-kde-wayland/bin".into()), theme: Some("Paper".into()) };
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
        let older: Settings = toml::from_str("theme = \"Midnight\"\nsomething_new = 3\n").unwrap();
        assert_eq!(older.theme.as_deref(), Some("Midnight"));
        assert_eq!(older.argyll_bin, None);
    }
}
