//! Display profiles: the ones installed for displays, the ones DisplayCraft
//! made, what each is, and which one KWin is using.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What a profile file says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub description: String,
    /// "YYYY-MM-DD HH:MM" local time, from the header (which is UTC)
    pub created: String,
    /// The display's white point, CIE xy. Only v2 profiles record it
    /// directly (v4 white points are D50 by definition), so None otherwise.
    pub white: Option<(f64, f64)>,
}

impl Info {
    /// Approximate correlated color temperature of the white point
    /// (McCamy's formula, good to a few K near the daylight locus)
    pub fn cct(&self) -> Option<f64> {
        let (x, y) = self.white?;
        let n = (x - 0.3320) / (0.1858 - y);
        Some(449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33)
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn s15f16(b: &[u8], at: usize) -> Option<f64> {
    Some(be32(b, at)? as i32 as f64 / 65536.0)
}

/// Read the description, creation date and white point of an ICC profile.
pub fn parse(b: &[u8]) -> Option<Info> {
    if b.get(36..40)? != b"acsp" {
        return None;
    }
    let version = *b.get(8)?;
    let created = chrono::NaiveDate::from_ymd_opt(be16(b, 24)? as i32, be16(b, 26)? as u32, be16(b, 28)? as u32)
        .and_then(|d| d.and_hms_opt(be16(b, 30)? as u32, be16(b, 32)? as u32, be16(b, 34)? as u32))
        .map(|utc| utc.and_utc().with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let tag = |sig: &[u8; 4]| -> Option<&[u8]> {
        let count = be32(b, 128)? as usize;
        (0..count.min(200)).find_map(|i| {
            let at = 132 + i * 12;
            if b.get(at..at + 4)? != sig {
                return None;
            }
            let (off, size) = (be32(b, at + 4)? as usize, be32(b, at + 8)? as usize);
            b.get(off..off.checked_add(size)?)
        })
    };
    let description = tag(b"desc").and_then(text).unwrap_or_default();
    let white = if version < 4 {
        tag(b"wtpt").filter(|t| t.starts_with(b"XYZ ")).and_then(|t| {
            let (x, y, z) = (s15f16(t, 8)?, s15f16(t, 12)?, s15f16(t, 16)?);
            let sum = x + y + z;
            (sum > 0.0).then(|| (x / sum, y / sum))
        })
    } else {
        None
    };
    Some(Info { description, created, white })
}

/// Text of a v2 'desc' or v4 'mluc' tag
fn text(t: &[u8]) -> Option<String> {
    match t.get(0..4)? {
        b"desc" => {
            let n = be32(t, 8)? as usize;
            let raw = t.get(12..12 + n)?;
            let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
            Some(String::from_utf8_lossy(&raw[..end]).into_owned())
        }
        b"mluc" => {
            // The first record; DisplayCraft and Argyll write one
            let (len, off) = (be32(t, 20)? as usize, be32(t, 24)? as usize);
            let units: Vec<u16> = t.get(off..off + len)?.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
        }
        _ => None,
    }
}

pub fn read(path: &Path) -> Option<Info> {
    parse(&std::fs::read(path).ok()?)
}

/// Where ArgyllCMS installs display profiles for the user
pub fn installed_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("color/icc/devices/display"))
}

/// .icc/.icm files in a directory, newest first
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("icc") || x.eq_ignore_ascii_case("icm")))
                .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
                .collect()
        })
        .unwrap_or_default();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The verdict of the newest verification report for a profile, from the
/// reports in `verify_dir` (named "<profile stem>-verify-<stamp>.txt")
pub fn last_verdict(verify_dir: &Path, profile: &Path) -> Option<String> {
    let stem = profile.file_stem()?.to_string_lossy().into_owned();
    let prefix = format!("{stem}-verify-");
    let newest = std::fs::read_dir(verify_dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(".txt"))
        // The stamp sorts by time
        .max()?;
    let text = std::fs::read_to_string(verify_dir.join(newest)).ok()?;
    text.lines().find_map(|l| l.strip_prefix("Verdict: ")).and_then(|v| v.split_whitespace().next()).map(str::to_string)
}

/// How KWin is managing an output's color
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputColor {
    /// The ICC profile set for the output, used when `icc_mode` is true
    pub icc_path: Option<PathBuf>,
    pub icc_mode: bool,
}

impl OutputColor {
    /// The profile KWin is actually using
    pub fn active(&self) -> Option<&Path> {
        self.icc_path.as_deref().filter(|_| self.icc_mode)
    }
}

/// Ask KWin, through kscreen-doctor, how `output` (e.g. "eDP-1") is managed.
/// Its JSON leaves out the profile source, so the text listing is read.
pub fn output_color(output: &str) -> Option<OutputColor> {
    let out = Command::new("kscreen-doctor").arg("-o").stdin(Stdio::null()).output().ok()?;
    parse_outputs(&String::from_utf8_lossy(&out.stdout), output)
}

fn parse_outputs(text: &str, output: &str) -> Option<OutputColor> {
    let mut found = None;
    let mut current = false;
    for line in text.lines() {
        // Colors and styles come as ANSI escapes
        let line = strip_ansi(line);
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Output:") {
            current = rest.split_whitespace().nth(1) == Some(output);
            if current {
                found = Some(OutputColor::default());
            }
            continue;
        }
        let Some(o) = found.as_mut().filter(|_| current) else { continue };
        if let Some(path) = line.strip_prefix("ICC profile:") {
            let path = path.trim();
            o.icc_path = (!path.is_empty() && path != "none").then(|| PathBuf::from(path));
        } else if let Some(source) = line.strip_prefix("Color profile source:") {
            o.icc_mode = source.trim() == "ICC";
        }
    }
    found
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ ... final byte in @..~
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) && c != '[' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v2 profile with a 'desc' and a 'wtpt' tag, no more
    fn profile(desc: &str, white: [f64; 3]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[8] = 2;
        b[36..40].copy_from_slice(b"acsp");
        for (i, v) in [2026u16, 10, 12, 20, 40, 53].iter().enumerate() {
            b[24 + i * 2..26 + i * 2].copy_from_slice(&v.to_be_bytes());
        }
        let mut desc_tag = b"desc\0\0\0\0".to_vec();
        desc_tag.extend(((desc.len() + 1) as u32).to_be_bytes());
        desc_tag.extend(desc.as_bytes());
        desc_tag.push(0);
        let mut wtpt = b"XYZ \0\0\0\0".to_vec();
        for v in white {
            wtpt.extend(((v * 65536.0).round() as i32).to_be_bytes());
        }
        let table_end = 128 + 4 + 2 * 12;
        let desc_at = table_end;
        let wtpt_at = desc_at + desc_tag.len();
        b.extend(2u32.to_be_bytes());
        for (sig, at, len) in [(b"desc", desc_at, desc_tag.len()), (b"wtpt", wtpt_at, wtpt.len())] {
            b.extend(sig);
            b.extend((at as u32).to_be_bytes());
            b.extend((len as u32).to_be_bytes());
        }
        b.extend(desc_tag);
        b.extend(wtpt);
        b
    }

    #[test]
    fn reads_description_date_and_white() {
        let info = parse(&profile("eDP-1 D55", [0.95497, 1.0, 0.91754])).unwrap();
        assert_eq!(info.description, "eDP-1 D55");
        let utc = chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap().and_hms_opt(20, 40, 53).unwrap().and_utc();
        assert_eq!(info.created, utc.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string());
        let (x, y) = info.white.unwrap();
        assert!((x - 0.3325).abs() < 1e-3 && (y - 0.3481).abs() < 1e-3, "{x} {y}");
        let cct = info.cct().unwrap();
        assert!((cct - 5500.0).abs() < 60.0, "{cct}");
        assert!(parse(b"not a profile").is_none());
    }

    #[test]
    fn reads_kscreen_doctor_listing() {
        let text = "Output: 1 eDP-1 59234731-3455\n\tenabled\n\x1b[01;32mICC profile:\x1b[0;0m /home/u/x.icc\n\
                    \tColor profile source: ICC\nOutput: 2 HDMI-A-1 abc\n\tICC profile: none\n\tColor profile source: sRGB\n";
        let o = parse_outputs(text, "eDP-1").unwrap();
        assert_eq!(o.active(), Some(Path::new("/home/u/x.icc")));
        let h = parse_outputs(text, "HDMI-A-1").unwrap();
        assert_eq!(h, OutputColor { icc_path: None, icc_mode: false });
        assert!(parse_outputs(text, "DP-3").is_none());
    }
}
