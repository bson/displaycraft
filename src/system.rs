//! What differs between operating systems in how DisplayCraft finds and
//! runs the ArgyllCMS it comes with. One implementation of `System` per
//! operating system, chosen when building; `current()` returns it.

use std::path::PathBuf;

use crate::argyll::WindowSystem;

/// One way of running the bundled ArgyllCMS, as offered in Preferences
pub struct Variant {
    pub label: &'static str,
    /// The display system it's told to use. None leaves it to ArgyllCMS.
    pub window_system: Option<WindowSystem>,
}

pub trait System: Sync {
    /// The bundled ArgyllCMS's variants, the default (None) first
    fn variants(&self) -> &'static [Variant];

    /// Where the bundled ArgyllCMS is. Its launcher names it in
    /// $DISPLAYCRAFT_ARGYLL_BIN.
    fn bundled_dir(&self) -> Option<PathBuf> {
        std::env::var_os("DISPLAYCRAFT_ARGYLL_BIN").map(PathBuf::from)
    }

    /// Why there's no bundled ArgyllCMS
    fn no_bundled(&self) -> &'static str;
}

#[cfg(target_os = "linux")]
struct Linux;

#[cfg(target_os = "linux")]
impl System for Linux {
    fn variants(&self) -> &'static [Variant] {
        &[
            Variant { label: "Bundled ArgyllCMS (auto)", window_system: None },
            Variant { label: "Bundled ArgyllCMS (KDE + Wayland)", window_system: Some(WindowSystem::Wayland) },
            Variant { label: "Bundled ArgyllCMS (GNOME, KDE + X11)", window_system: Some(WindowSystem::X11) },
        ]
    }

    /// AppRun sets $DISPLAYCRAFT_ARGYLL_BIN. Run from a source checkout,
    /// the same fork installed in /opt stands in for it.
    fn bundled_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("DISPLAYCRAFT_ARGYLL_BIN") {
            return Some(PathBuf::from(dir));
        }
        let opt = PathBuf::from("/opt/argyll-kde-wayland/bin");
        opt.join("dispcal").exists().then_some(opt)
    }

    fn no_bundled(&self) -> &'static str {
        "There's no bundled ArgyllCMS: DisplayCraft wasn't started from its AppImage"
    }
}

/// ArgyllCMS has one way of showing test patches here, so there's nothing
/// to choose
#[cfg(not(target_os = "linux"))]
struct Single;

#[cfg(not(target_os = "linux"))]
impl System for Single {
    fn variants(&self) -> &'static [Variant] {
        &[Variant { label: "Bundled ArgyllCMS", window_system: None }]
    }

    fn no_bundled(&self) -> &'static str {
        "There's no bundled ArgyllCMS: DisplayCraft wasn't started from its installed package"
    }
}

pub fn current() -> &'static dyn System {
    #[cfg(target_os = "linux")]
    return &Linux;
    #[cfg(not(target_os = "linux"))]
    return &Single;
}
