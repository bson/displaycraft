//! Whether the user may open the measuring instruments that are plugged in,
//! and installing ArgyllCMS's udev rule when they may not.
//!
//! Instruments are opened through /dev/bus/usb, which needs permission. A
//! distribution's ArgyllCMS or colord package usually grants it with a udev
//! rule; on a system with neither (e.g. running the AppImage), an instrument
//! shows up in USB but ArgyllCMS can't use it. The rule DisplayCraft
//! installs is ArgyllCMS's own 55-Argyll.rules, kept in assets/udev.

use std::path::{Path, PathBuf};

pub const RULES: &str = include_str!("../assets/udev/55-Argyll.rules");
pub const RULES_NAME: &str = "55-Argyll.rules";

/// An instrument that's plugged in but can't be opened
#[derive(Clone, Debug, PartialEq)]
pub struct Blocked {
    /// What the device calls itself, e.g. "i1Display3"
    pub name: String,
    /// e.g. /dev/bus/usb/001/017
    pub node: PathBuf,
}

/// The vendor and product IDs a rules file covers
pub fn ids(rules: &str) -> Vec<(String, String)> {
    let value = |line: &str, key: &str| -> Option<String> {
        let rest = line.split(key).nth(1)?.strip_prefix("==\"")?;
        Some(rest.split('"').next()?.to_ascii_lowercase())
    };
    rules
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| Some((value(l, "ATTRS{idVendor}")?, value(l, "ATTRS{idProduct}")?)))
        .collect()
}

/// Instruments under `sysfs` (normally /sys/bus/usb/devices) that the rules
/// cover but whose device node under `dev` (normally /dev/bus/usb) can't be
/// opened for reading and writing
pub fn blocked_in(sysfs: &Path, dev: &Path, known: &[(String, String)]) -> Vec<Blocked> {
    let Ok(entries) = std::fs::read_dir(sysfs) else { return Vec::new() };
    let read = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).ok().map(|s| s.trim().to_string());
    let mut found = Vec::new();
    for e in entries.flatten() {
        let d = e.path();
        let (Some(vid), Some(pid)) = (read(&d, "idVendor"), read(&d, "idProduct")) else { continue };
        if !known.iter().any(|(v, p)| *v == vid.to_ascii_lowercase() && *p == pid.to_ascii_lowercase()) {
            continue;
        }
        let (Some(bus), Some(num)) = (read(&d, "busnum"), read(&d, "devnum")) else { continue };
        let (Ok(bus), Ok(num)) = (bus.parse::<u32>(), num.parse::<u32>()) else { continue };
        let node = dev.join(format!("{bus:03}")).join(format!("{num:03}"));
        // Opening the node claims nothing; it only tests the permission
        if std::fs::OpenOptions::new().read(true).write(true).open(&node).is_err() {
            let name = read(&d, "product").unwrap_or_else(|| format!("USB device {vid}:{pid}"));
            found.push(Blocked { name, node });
        }
    }
    found
}

pub fn blocked() -> Vec<Blocked> {
    blocked_in(Path::new("/sys/bus/usb/devices"), Path::new("/dev/bus/usb"), &ids(RULES))
}

/// The shell command pkexec runs to install `rules` (a file the user can
/// read) and apply it to devices already plugged in
pub fn install_command(rules: &Path) -> String {
    let quoted = format!("'{}'", rules.display().to_string().replace('\'', r"'\''"));
    format!(
        "install -m 0644 {quoted} /etc/udev/rules.d/{RULES_NAME} && udevadm control --reload-rules && \
         udevadm trigger --subsystem-match=usb --action=add"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_instruments() {
        let ids = ids(RULES);
        assert!(ids.len() > 25, "{}", ids.len());
        assert!(ids.contains(&("0765".into(), "5020".into())), "i1Display Pro");
    }

    #[test]
    fn finds_an_instrument_it_cannot_open() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/test-tmp-usb-{}", std::process::id()));
        let (sys, dev) = (root.join("sys"), root.join("dev"));
        for (dir, vid, pid, bus, num) in [("1-1", "0765", "5020", "1", "17"), ("1-2", "046d", "c52b", "1", "3")] {
            let d = sys.join(dir);
            std::fs::create_dir_all(&d).unwrap();
            for (f, v) in [("idVendor", vid), ("idProduct", pid), ("busnum", bus), ("devnum", num), ("product", "Thing")] {
                std::fs::write(d.join(f), format!("{v}\n")).unwrap();
            }
        }
        // No device node at all: can't be opened
        let blocked = blocked_in(&sys, &dev, &ids(RULES));
        assert_eq!(blocked, vec![Blocked { name: "Thing".into(), node: dev.join("001/017") }], "only the instrument counts");
        // One the user may open
        std::fs::create_dir_all(dev.join("001")).unwrap();
        std::fs::write(dev.join("001/017"), "").unwrap();
        assert!(blocked_in(&sys, &dev, &ids(RULES)).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
        assert!(install_command(Path::new("/tmp/it's here/r")).contains(r"'/tmp/it'\''s here/r'"));
    }
}
