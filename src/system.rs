//! What differs between operating systems: finding and running the
//! ArgyllCMS DisplayCraft comes with, where files go, which profile a
//! display uses, and opening files and links. One
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

    /// Where DisplayCraft keeps calibrations, profiles and reports
    fn data_dir(&self) -> Option<PathBuf>;

    /// Where DisplayCraft keeps its settings
    fn config_dir(&self) -> Option<PathBuf>;

    /// Where display profiles installed for the user are
    fn installed_profiles_dir(&self) -> Option<PathBuf>;

    /// Folders, besides ArgyllCMS's own ref folder, where ArgyllCMS and
    /// DisplayCAL keep colorimeter corrections, most specific first
    fn correction_dirs(&self) -> Vec<PathBuf>;

    /// What decides which profile a display uses, for the page's wording
    fn color_manager(&self) -> &'static str;

    /// The profile display `display` uses (ArgyllCMS's `-d` value; `output`
    /// is its output name, e.g. "eDP-1"): Some(None) if none, None if that
    /// couldn't be found out. By default ArgyllCMS is asked (`dispwin -W`).
    fn profile_in_use(&self, argyll_bin: &Path, display: &str, _output: &str) -> Option<Option<PathBuf>> {
        crate::argyll::installed_profile(argyll_bin, display)
    }

    /// Said when a display uses no profile
    fn without_profile(&self) -> &'static str {
        ""
    }

    /// Called when an ArgyllCMS program that may have changed display
    /// `display`'s calibration curves has ended, however it ended
    fn after_curves_changed(&self, _argyll_bin: &Path, _display: &str) {}
}

/// Run `cmd` without a console window of its own. On Windows a console
/// program started from a GUI program gets one, which would flash up for
/// every ArgyllCMS program DisplayCraft runs. Nothing to do elsewhere.
pub fn no_console(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Make `child` end when DisplayCraft does, however DisplayCraft ends. On
/// Windows a started program outlives its parent, so each is put in a job
/// object that kills its processes when DisplayCraft's handle to it closes,
/// which Windows does at exit even after a crash. Elsewhere a Job that's
/// dropped cancels its program (argyll::Job's Drop), and nothing more is
/// done here.
pub fn adopt(child: &std::process::Child) {
    #[cfg(windows)]
    windows_job::adopt(child);
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(windows)]
mod windows_job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    /// Never closed: closing is what kills the processes, at exit
    struct Job(HANDLE);
    // A handle is just a number to the kernel, usable from any thread
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    pub fn adopt(child: &std::process::Child) {
        let job = JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            if SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size) == 0 {
                CloseHandle(h);
                return None;
            }
            Some(Job(h))
        });
        if let Some(job) = job {
            unsafe {
                AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE);
            }
        }
    }
}

// Windows' paths are joined a component at a time: a "/" inside one
// stays as it is, and the path comes out with both separators

/// The user's roaming application data folder (AppData\Roaming)
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn roaming() -> Option<PathBuf> {
    home().map(|h| h.join("AppData").join("Roaming"))
}

/// One of Windows' own folders, as Windows names it to every program (not
/// a user setting), with its usual place if it doesn't
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn windows_folder(var: &str, usual: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(usual))
}

fn home() -> Option<PathBuf> {
    std::env::home_dir()
}

#[cfg(target_os = "linux")]
struct Linux;

#[cfg(target_os = "linux")]
impl System for Linux {
    fn variants(&self) -> &'static [Variant] {
        &[
            Variant { label: "Bundled ArgyllCMS (auto)", window_system: None },
            Variant { label: "Bundled ArgyllCMS (KDE + Wayland)", window_system: Some(WindowSystem::Wayland) },
            Variant { label: "Bundled ArgyllCMS (GNOME, KDE + Xorg)", window_system: Some(WindowSystem::X11) },
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

    // The XDG base directories, as the desktop session sets them
    fn data_dir(&self) -> Option<PathBuf> {
        xdg("XDG_DATA_HOME", ".local/share").map(|d| d.join("displaycraft"))
    }

    fn config_dir(&self) -> Option<PathBuf> {
        xdg("XDG_CONFIG_HOME", ".config").map(|d| d.join("displaycraft"))
    }

    /// ucmm's, which ArgyllCMS installs into (and colord reads)
    fn installed_profiles_dir(&self) -> Option<PathBuf> {
        xdg("XDG_CONFIG_HOME", ".config").map(|d| d.join("color/icc/devices/display"))
    }

    fn correction_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(d) = xdg("XDG_DATA_HOME", ".local/share") {
            dirs.push(d.join("ArgyllCMS"));
            // DisplayCAL downloads the vendor corrections here
            dirs.push(d.join("DisplayCAL/dl"));
        }
        let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        for d in data_dirs.split(':').filter(|d| !d.is_empty()) {
            dirs.push(Path::new(d).join("ArgyllCMS"));
        }
        dirs.push("/usr/share/color/argyll/ref".into());
        dirs
    }

    fn color_manager(&self) -> &'static str {
        "KDE"
    }

    /// KWin knows which profile it's using, and whether it's using one at
    /// all (its ICC mode), which colord and ucmm can't say
    fn profile_in_use(&self, _argyll_bin: &Path, _display: &str, output: &str) -> Option<Option<PathBuf>> {
        crate::profiles::output_color(output).map(|c| c.active().map(Path::to_path_buf))
    }

    fn without_profile(&self) -> &'static str {
        "KDE treats the display as sRGB."
    }
}

#[cfg(target_os = "linux")]
fn xdg(var: &str, default: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home().map(|h| h.join(default)))
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

    fn data_dir(&self) -> Option<PathBuf> {
        home().map(|h| h.join("Library/Application Support/DisplayCraft"))
    }

    fn config_dir(&self) -> Option<PathBuf> {
        self.data_dir()
    }

    /// Where ArgyllCMS installs a profile for the user, and ColorSync looks
    fn installed_profiles_dir(&self) -> Option<PathBuf> {
        home().map(|h| h.join("Library/ColorSync/Profiles"))
    }

    fn correction_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(h) = home() {
            dirs.push(h.join("Library/Application Support/ArgyllCMS"));
            dirs.push(h.join("Library/Application Support/DisplayCAL/dl"));
        }
        dirs.push("/Library/Application Support/ArgyllCMS".into());
        dirs
    }

    fn color_manager(&self) -> &'static str {
        "macOS"
    }

    /// macOS 27 workaround, hopefully temporary: when a program that set the
    /// curves exits, macOS loads an all-zero table instead of the profile's
    /// calibration, leaving the display black. ArgyllCMS avoids that when it
    /// exits normally, but not when it's killed (Cancel), so load the
    /// profile's calibration in the way that lasts (`dispwin -L`). Waits for
    /// it, so that it can't race the next stage's program.
    fn after_curves_changed(&self, argyll_bin: &Path, display: &str) {
        let _ = crate::argyll::command(argyll_bin.join("dispwin"))
            .args(["-d", display, "-L"])
            .stdin(std::process::Stdio::null())
            .output();
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

    /// In the argyll\bin folder beside DisplayCraft's program
    fn bundled_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("DISPLAYCRAFT_ARGYLL_BIN") {
            return Some(PathBuf::from(dir));
        }
        let dir = std::env::current_exe().ok()?.parent()?.join("argyll").join("bin");
        dir.join("dispcal.exe").exists().then_some(dir)
    }

    fn no_bundled(&self) -> &'static str {
        "There's no bundled ArgyllCMS: DisplayCraft wasn't started from its installed package"
    }

    fn open(&self, target: &OsStr) {
        // start's first quoted argument is a window title, hence the ""
        let _ = no_console(Command::new("cmd").args(["/C", "start", ""]).arg(target)).spawn();
    }

    fn data_dir(&self) -> Option<PathBuf> {
        roaming().map(|r| r.join("DisplayCraft"))
    }

    fn config_dir(&self) -> Option<PathBuf> {
        self.data_dir()
    }

    fn installed_profiles_dir(&self) -> Option<PathBuf> {
        Some(windows_folder("SystemRoot", r"C:\Windows").join("System32").join("spool").join("drivers").join("color"))
    }

    fn correction_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(r) = roaming() {
            dirs.push(r.join("ArgyllCMS"));
            dirs.push(r.join("DisplayCAL").join("dl"));
        }
        dirs.push(windows_folder("ProgramData", r"C:\ProgramData").join("ArgyllCMS"));
        dirs
    }

    fn color_manager(&self) -> &'static str {
        "Windows"
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
