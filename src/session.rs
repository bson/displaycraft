//! Telling the user something when DisplayCraft can't open its own window.
//! (Whether ArgyllCMS can run in this session is decided by ArgyllCMS
//! itself, with dispwin -Q: see argyll::check_platform.)

/// Our window needs Wayland or X11, which may be what's missing. Tries the
/// desktop's dialog tools, and always prints.
pub fn report_without_window(message: &str) {
    eprintln!("{message}");
    for (tool, args) in [("kdialog", vec!["--title", "DisplayCraft", "--error"]), ("zenity", vec!["--error", "--title=DisplayCraft", "--text"])] {
        if std::process::Command::new(tool).args(&args).arg(message).status().is_ok() {
            return;
        }
    }
}
