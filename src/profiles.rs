//! Display profiles: the ones installed for displays, the ones DisplayCraft
//! made, what each is, and which one KWin is using.

use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
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
    /// Has a device-to-PCS table (A2B0), i.e. is a LUT profile rather than
    /// curves and a matrix
    pub lut: bool,
    /// The device model description (dmdd), where DisplayCraft records the
    /// output a profile was made for (colprof -M)
    pub model: String,
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
    let model = tag(b"dmdd").and_then(text).unwrap_or_default();
    let lut = tag(b"A2B0").is_some();
    let white = if version < 4 {
        tag(b"wtpt").filter(|t| t.starts_with(b"XYZ ")).and_then(|t| {
            let (x, y, z) = (s15f16(t, 8)?, s15f16(t, 12)?, s15f16(t, 16)?);
            let sum = x + y + z;
            (sum > 0.0).then(|| (x / sum, y / sum))
        })
    } else {
        None
    };
    Some(Info { description, created, white, lut, model })
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

/// What a profile's graphs show
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curves {
    /// The calibration (vcgt), red, green and blue, 256 samples each
    pub calibration: Option<[Vec<f64>; 3]>,
    /// The tonal response curves (TRC), 256 samples each
    pub tone: Option<[Vec<f64>; 3]>,
    /// The display's red, green and blue primaries, CIE xy, undoing the
    /// profile's adaptation to D50
    pub primaries: Option<[(f64, f64); 3]>,
    /// Its white, CIE xy
    pub white: Option<(f64, f64)>,
}

const SAMPLES: usize = 256;

/// The tag table entry for `sig`
fn find_tag<'a>(b: &'a [u8], sig: &[u8; 4]) -> Option<&'a [u8]> {
    let count = be32(b, 128)? as usize;
    (0..count.min(200)).find_map(|i| {
        let at = 132 + i * 12;
        if b.get(at..at + 4)? != sig {
            return None;
        }
        let (off, size) = (be32(b, at + 4)? as usize, be32(b, at + 8)? as usize);
        b.get(off..off.checked_add(size)?)
    })
}

/// Sample a 'curv' or 'para' tag at SAMPLES points
fn sample_trc(t: &[u8]) -> Option<Vec<f64>> {
    let at = |i: usize| i as f64 / (SAMPLES - 1) as f64;
    match t.get(0..4)? {
        b"curv" => {
            let n = be32(t, 8)? as usize;
            match n {
                0 => Some((0..SAMPLES).map(at).collect()),
                1 => {
                    let g = be16(t, 12)? as f64 / 256.0;
                    Some((0..SAMPLES).map(|i| at(i).powf(g)).collect())
                }
                _ => {
                    let table: Vec<f64> = (0..n).map(|i| be16(t, 12 + 2 * i).map(|v| v as f64 / 65535.0)).collect::<Option<_>>()?;
                    Some((0..SAMPLES).map(|i| interpolate(&table, at(i))).collect())
                }
            }
        }
        b"para" => {
            let kind = be16(t, 8)?;
            let count = [1, 3, 4, 5, 7].get(kind as usize)?;
            let p: Vec<f64> = (0..*count).map(|i| s15f16(t, 12 + 4 * i)).collect::<Option<_>>()?;
            let f = |x: f64| -> f64 {
                let g = p[0];
                match kind {
                    0 => x.powf(g),
                    1 => if x >= -p[2] / p[1] { (p[1] * x + p[2]).max(0.0).powf(g) } else { 0.0 },
                    2 => if x >= -p[2] / p[1] { (p[1] * x + p[2]).max(0.0).powf(g) + p[3] } else { p[3] },
                    3 => if x >= p[4] { (p[1] * x + p[2]).max(0.0).powf(g) } else { p[3] * x },
                    _ => if x >= p[4] { (p[1] * x + p[2]).max(0.0).powf(g) + p[5] } else { p[3] * x + p[6] },
                }
            };
            Some((0..SAMPLES).map(|i| f(at(i)).clamp(0.0, 1.0)).collect())
        }
        _ => None,
    }
}

fn interpolate(table: &[f64], x: f64) -> f64 {
    let pos = x.clamp(0.0, 1.0) * (table.len() - 1) as f64;
    let i = (pos as usize).min(table.len() - 2);
    let f = pos - i as f64;
    table[i] + f * (table[i + 1] - table[i])
}

/// Sample a 'vcgt' tag, table or formula form
fn sample_vcgt(t: &[u8]) -> Option<[Vec<f64>; 3]> {
    if t.get(0..4)? != b"vcgt" {
        return None;
    }
    let at = |i: usize| i as f64 / (SAMPLES - 1) as f64;
    match be32(t, 8)? {
        0 => {
            let (channels, n, size) = (be16(t, 12)? as usize, be16(t, 14)? as usize, be16(t, 16)? as usize);
            if n < 2 || !(size == 1 || size == 2) {
                return None;
            }
            let entry = |c: usize, i: usize| -> Option<f64> {
                let at = 18 + (c * n + i) * size;
                Some(if size == 1 { *t.get(at)? as f64 / 255.0 } else { be16(t, at)? as f64 / 65535.0 })
            };
            let channel = |c: usize| -> Option<Vec<f64>> {
                let table: Vec<f64> = (0..n).map(|i| entry(c.min(channels - 1), i)).collect::<Option<_>>()?;
                Some((0..SAMPLES).map(|i| interpolate(&table, at(i))).collect())
            };
            Some([channel(0)?, channel(1)?, channel(2)?])
        }
        1 => {
            // Gamma, minimum and maximum per channel
            let channel = |c: usize| -> Option<Vec<f64>> {
                let (g, lo, hi) = (s15f16(t, 12 + 12 * c)?, s15f16(t, 16 + 12 * c)?, s15f16(t, 20 + 12 * c)?);
                Some((0..SAMPLES).map(|i| lo + (hi - lo) * at(i).powf(g)).collect())
            };
            Some([channel(0)?, channel(1)?, channel(2)?])
        }
        _ => None,
    }
}

type M3 = [[f64; 3]; 3];

fn mul(a: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|r| a[r][0] * v[0] + a[r][1] * v[1] + a[r][2] * v[2])
}

fn mat_mul(a: &M3, b: &M3) -> M3 {
    [0, 1, 2].map(|r| [0, 1, 2].map(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn invert(m: &M3) -> Option<M3> {
    let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if d.abs() < 1e-12 {
        return None;
    }
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    Some([
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ])
}

const BRADFORD: M3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

/// Von Kries adaptation in the cone space `cone`, from white `from` to
/// white `to` (XYZ)
fn adaptation(cone: &M3, from: [f64; 3], to: [f64; 3]) -> Option<M3> {
    let (s, d) = (mul(cone, from), mul(cone, to));
    let scale: M3 = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    Some(mat_mul(&invert(cone)?, &mat_mul(&scale, cone)))
}

fn xyz_tag(b: &[u8], sig: &[u8; 4]) -> Option<[f64; 3]> {
    let t = find_tag(b, sig).filter(|t| t.starts_with(b"XYZ "))?;
    Some([s15f16(t, 8)?, s15f16(t, 12)?, s15f16(t, 16)?])
}

fn xy_of(c: [f64; 3]) -> Option<(f64, f64)> {
    let s = c[0] + c[1] + c[2];
    (s > 0.0).then(|| (c[0] / s, c[1] / s))
}

/// Read the calibration, tonal response and primaries for graphs
pub fn curves(b: &[u8]) -> Option<Curves> {
    if b.get(36..40)? != b"acsp" {
        return None;
    }
    let tone = match (find_tag(b, b"rTRC"), find_tag(b, b"gTRC"), find_tag(b, b"bTRC")) {
        (Some(r), Some(g), Some(bl)) => Some([sample_trc(r)?, sample_trc(g)?, sample_trc(bl)?]),
        _ => None,
    };
    let calibration = find_tag(b, b"vcgt").and_then(sample_vcgt);
    // The colorants are adapted to D50. ICC v4's 'chad' is that adaptation.
    // Argyll's 'arts' is not: it's the cone space the adaptation was done
    // in (Bradford, or a sharpened variant), so the adaptation is rebuilt
    // from it and the (v2, absolute) white point; Bradford if neither.
    const D50: [f64; 3] = [0.9642, 1.0, 0.8249];
    let wtpt = xyz_tag(b, b"wtpt");
    let matrix = |sig: &[u8; 4]| -> Option<M3> {
        let t = find_tag(b, sig).filter(|t| t.starts_with(b"sf32"))?;
        let v: Vec<f64> = (0..9).map(|i| s15f16(t, 8 + 4 * i)).collect::<Option<_>>()?;
        Some([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
    };
    let v2 = *b.get(8)? < 4;
    let adapt = matrix(b"chad").or_else(|| {
        let cone = matrix(b"arts").unwrap_or(BRADFORD);
        v2.then(|| adaptation(&cone, wtpt?, D50)).flatten()
    });
    let undo = adapt.as_ref().and_then(invert);
    let absolute = |c: [f64; 3]| match &undo {
        Some(m) => mul(m, c),
        None => c,
    };
    let primaries = match (xyz_tag(b, b"rXYZ"), xyz_tag(b, b"gXYZ"), xyz_tag(b, b"bXYZ")) {
        (Some(r), Some(g), Some(bl)) => Some([xy_of(absolute(r))?, xy_of(absolute(g))?, xy_of(absolute(bl))?]),
        _ => None,
    };
    // A v2 white point is absolute already; a v4 one is D50 by definition,
    // so undo the adaptation for it too
    let white = match (wtpt, v2) {
        (Some(w), true) => xy_of(w),
        (Some(w), false) => xy_of(absolute(w)),
        _ => None,
    };
    Some(Curves { calibration, tone, primaries, white })
}

pub fn read_curves(path: &Path) -> Option<Curves> {
    curves(&std::fs::read(path).ok()?)
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

#[cfg(target_os = "linux")]
/// How KWin is managing an output's color
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputColor {
    /// The ICC profile set for the output, used when `icc_mode` is true
    pub icc_path: Option<PathBuf>,
    pub icc_mode: bool,
}

#[cfg(target_os = "linux")]
impl OutputColor {
    /// The profile KWin is actually using
    pub fn active(&self) -> Option<&Path> {
        self.icc_path.as_deref().filter(|_| self.icc_mode)
    }
}

#[cfg(target_os = "linux")]
/// Ask KWin, through kscreen-doctor, how `output` (e.g. "eDP-1") is managed.
/// Its JSON leaves out the profile source, so the text listing is read.
pub fn output_color(output: &str) -> Option<OutputColor> {
    let out = Command::new("kscreen-doctor").arg("-o").stdin(Stdio::null()).output().ok()?;
    parse_outputs(&String::from_utf8_lossy(&out.stdout), output)
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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
        assert!(!info.lut);
        assert!(parse(b"not a profile").is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
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
