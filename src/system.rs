//! What differs between operating systems: finding and running the
//! ArgyllCMS DisplayCraft comes with, and opening files and links. One
//! implementation of `System` per operating system, chosen when building;
//! `current()` returns it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

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

    /// Open a file or URL with the desktop's default application, without
    /// waiting for it
    fn open(&self, target: &OsStr);

    /// A command running `program` with its output unbuffered, so that
    /// progress and prompts (which don't end with a newline) can be read as
    /// they're printed. The bundled ArgyllCMS makes its output unbuffered
    /// itself when it isn't a terminal.
    fn live_output(&self, program: &Path) -> Command {
        Command::new(program)
    }
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

    fn open(&self, target: &OsStr) {
        let _ = Command::new("xdg-open").arg(target).spawn();
    }

    /// Through stdbuf, which an ArgyllCMS other than the bundled one needs,
    /// and which does no harm with the bundled one
    fn live_output(&self, program: &Path) -> Command {
        let mut cmd = Command::new("stdbuf");
        cmd.arg("-o0").arg("-e0").arg(program);
        cmd
    }
}

#[cfg(target_os = "macos")]
struct MacOs;

#[cfg(target_os = "macos")]
impl System for MacOs {
    fn variants(&self) -> &'static [Variant] {
        &[Variant { label: "Bundled ArgyllCMS", window_system: None }]
    }

    /// Inside DisplayCraft.app, beside the executable in Contents/MacOS:
    /// Contents/Resources/argyll/bin. Run from a source checkout, an
    /// installed DisplayCraft.app's stands in for it, as /opt's does on
    /// Linux.
    fn bundled_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("DISPLAYCRAFT_ARGYLL_BIN") {
            return Some(PathBuf::from(dir));
        }
        let inside = |app: &std::path::Path| Some(app.join("Contents/Resources/argyll/bin")).filter(|d| d.join("dispcal").exists());
        let exe = std::env::current_exe().ok()?;
        let own = exe.parent()?.parent()?.parent()?.to_path_buf();
        let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications/DisplayCraft.app"));
        [Some(own), Some(PathBuf::from("/Applications/DisplayCraft.app")), home].into_iter().flatten().find_map(|app| inside(&app))
    }

    fn no_bundled(&self) -> &'static str {
        "There's no bundled ArgyllCMS: DisplayCraft wasn't started from DisplayCraft.app"
    }

    fn open(&self, target: &OsStr) {
        let _ = Command::new("open").arg(target).spawn();
    }
}

/// Windows, for now: ArgyllCMS has one way of showing test patches, so
/// there's nothing to choose
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct Single;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl System for Single {
    fn variants(&self) -> &'static [Variant] {
        &[Variant { label: "Bundled ArgyllCMS", window_system: None }]
    }

    fn no_bundled(&self) -> &'static str {
        "There's no bundled ArgyllCMS: DisplayCraft wasn't started from its installed package"
    }

    fn open(&self, target: &OsStr) {
        // start's first quoted argument is a window title, hence the ""
        let _ = Command::new("cmd").args(["/C", "start", ""]).arg(target).spawn();
    }
}

pub fn current() -> &'static dyn System {
    #[cfg(target_os = "linux")]
    return &Linux;
    #[cfg(target_os = "macos")]
    return &MacOs;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return &Single;
}
