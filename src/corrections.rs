//! Colorimeter corrections: .ccss (spectral samples) and .ccmx (matrix)
//! files, passed to Argyll tools with -X.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Correction {
    pub path: PathBuf,
    /// e.g. "LCD White LED IPS"
    pub technology: String,
    /// The display it was made for, or the file's descriptor
    pub display: String,
}

impl Correction {
    pub fn label(&self) -> String {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match (self.technology.is_empty(), self.display.is_empty()) {
            (false, false) => format!("{} — {}", self.technology, self.display),
            (false, true) => self.technology.clone(),
            (true, false) => self.display.clone(),
            (true, true) => name,
        }
    }
}

/// Where Argyll and DisplayCAL keep corrections, most specific first.
fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    let mut dirs = Vec::new();
    if let Some(d) = &data_home {
        dirs.push(d.join("ArgyllCMS"));
        // DisplayCAL downloads the vendor corrections here
        dirs.push(d.join("DisplayCAL/dl"));
    }
    for d in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(d).join("ArgyllCMS"));
    }
    dirs.push("/usr/share/color/argyll/ref".into());
    dirs
}

/// Correction files found in the usual places, one per file name.
pub fn find() -> Vec<Correction> {
    let mut found: Vec<Correction> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in search_dirs() {
        // One level of subdirectories, as DisplayCAL sorts them by instrument
        let mut files = list(&dir);
        for sub in subdirs(&dir) {
            files.extend(list(&sub));
        }
        for f in files {
            let name = f.file_name().map(|n| n.to_os_string());
            if name.is_some_and(|n| seen.insert(n)) {
                if let Some(c) = read(&f) {
                    found.push(c);
                }
            }
        }
    }
    found.sort_by(|a, b| a.label().to_lowercase().cmp(&b.label().to_lowercase()));
    found
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

fn list(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ccss") || x.eq_ignore_ascii_case("ccmx")))
                .collect()
        })
        .unwrap_or_default()
}

/// Read a correction's labels from its CGATS header.
pub fn read(path: &Path) -> Option<Correction> {
    let text = std::fs::read_to_string(path).ok()?;
    let first = text.lines().next()?.trim();
    if first != "CCSS" && first != "CCMX" {
        return None;
    }
    let keyword = |key: &str| -> String {
        text.lines()
            .take_while(|l| !l.starts_with("BEGIN_DATA"))
            .find_map(|l| l.strip_prefix(key).filter(|rest| rest.starts_with(' ')))
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let descriptor = keyword("DESCRIPTOR");
    let mut display = keyword("DISPLAY");
    if display.is_empty() && descriptor != "Not specified" {
        display = descriptor;
    }
    Some(Correction { path: path.to_path_buf(), technology: keyword("TECHNOLOGY"), display })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_labels_from_header() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/test-tmp-cc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("WLEDFamily_07Feb11.ccss");
        std::fs::write(&f, "CCSS   \nORIGINATOR \"X-Rite\"\nDISPLAY \"WLED AC LG Samsung\"\nTECHNOLOGY \"LCD White LED IPS\"\n\
            DISPLAY_TYPE_REFRESH \"NO\"\nDESCRIPTOR \"Not specified\"\nBEGIN_DATA_FORMAT\nSAMPLE_ID\nEND_DATA_FORMAT\n").unwrap();
        let c = read(&f).unwrap();
        assert_eq!(c.technology, "LCD White LED IPS");
        assert_eq!(c.display, "WLED AC LG Samsung", "DISPLAY_TYPE_REFRESH isn't mistaken for DISPLAY");
        assert_eq!(c.label(), "LCD White LED IPS — WLED AC LG Samsung");
        std::fs::write(dir.join("not.ccss"), "CTI3\n").unwrap();
        assert!(read(&dir.join("not.ccss")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
