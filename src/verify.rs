//! Profile verification: measure a patch set through a profile's calibration
//! and compare it with what the profile predicts.
//!
//! The work is done by ArgyllCMS: `fakeread` predicts each patch from the
//! profile, `dispread` measures them, and `colverify` reports the CIEDE2000
//! difference per patch. Both readings are normalised to their own white, so
//! a different backlight setting since calibrating doesn't count as error.

use std::fmt::Write as _;
use std::path::Path;

/// A verification patch: device RGB in percent and a name for reports.
pub struct Patch {
    pub rgb: [f64; 3],
    pub name: String,
}

/// The patch set: a 21 step grayscale, primaries and secondaries at full and
/// half level, and the 24 ColorChecker colors as sRGB values.
pub fn patches() -> Vec<Patch> {
    let mut p = Vec::new();
    for i in 0..=20 {
        let v = i as f64 * 5.0;
        p.push(Patch { rgb: [v, v, v], name: format!("Gray {v:.0}%") });
    }
    let colors = [
        ("Red", [1.0, 0.0, 0.0]),
        ("Green", [0.0, 1.0, 0.0]),
        ("Blue", [0.0, 0.0, 1.0]),
        ("Cyan", [0.0, 1.0, 1.0]),
        ("Magenta", [1.0, 0.0, 1.0]),
        ("Yellow", [1.0, 1.0, 0.0]),
    ];
    for level in [100.0, 50.0] {
        for (name, c) in colors {
            p.push(Patch { rgb: c.map(|x| x * level), name: format!("{name} {level:.0}%") });
        }
    }
    // ColorChecker sRGB (8 bit), BabelColor averages
    const CHECKER: [(&str, [u8; 3]); 24] = [
        ("Dark skin", [115, 82, 68]),
        ("Light skin", [194, 150, 130]),
        ("Blue sky", [98, 122, 157]),
        ("Foliage", [87, 108, 67]),
        ("Blue flower", [133, 128, 177]),
        ("Bluish green", [103, 189, 170]),
        ("Orange", [214, 126, 44]),
        ("Purplish blue", [80, 91, 166]),
        ("Moderate red", [193, 90, 99]),
        ("Purple", [94, 60, 108]),
        ("Yellow green", [157, 188, 64]),
        ("Orange yellow", [224, 163, 46]),
        ("Blue", [56, 61, 150]),
        ("Green", [70, 148, 73]),
        ("Red", [175, 54, 60]),
        ("Yellow", [231, 199, 31]),
        ("Magenta", [187, 86, 149]),
        ("Cyan", [8, 133, 161]),
        ("White 9.5", [243, 243, 242]),
        ("Neutral 8", [200, 200, 200]),
        ("Neutral 6.5", [160, 160, 160]),
        ("Neutral 5", [122, 122, 121]),
        ("Neutral 3.5", [85, 85, 85]),
        ("Black 2", [52, 52, 52]),
    ];
    for (name, c) in CHECKER {
        p.push(Patch { rgb: c.map(|x| x as f64 / 2.55), name: format!("ColorChecker {name}") });
    }
    p
}

/// Write the patches as an Argyll .ti1 (device values only).
pub fn write_ti1(path: &Path, patches: &[Patch]) -> std::io::Result<()> {
    let mut s = String::from(
        "CTI1\n\nDESCRIPTOR \"DisplayCraft verification patches\"\nORIGINATOR \"DisplayCraft\"\nCOLOR_REP \"RGB\"\n\n\
         NUMBER_OF_FIELDS 7\nBEGIN_DATA_FORMAT\nSAMPLE_ID RGB_R RGB_G RGB_B XYZ_X XYZ_Y XYZ_Z\nEND_DATA_FORMAT\n\n",
    );
    let _ = writeln!(s, "NUMBER_OF_SETS {}\nBEGIN_DATA", patches.len());
    for (i, p) in patches.iter().enumerate() {
        let _ = writeln!(s, "{} {:.4} {:.4} {:.4} 0 0 0", i + 1, p.rgb[0], p.rgb[1], p.rgb[2]);
    }
    s.push_str("END_DATA\n");
    std::fs::write(path, s)
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatchResult {
    pub index: usize,
    pub expected_lab: [f64; 3],
    pub measured_lab: [f64; 3],
    pub de: f64,
}

/// Parse `colverify -v 2 -k` per patch lines:
/// "12: 54.29 80.81 69.90 <=> 54.10 81.02 69.77  de 0.346"
pub fn parse_colverify(text: &str) -> Vec<PatchResult> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((id, rest)) = line.trim().split_once(": ") else { continue };
        let Ok(index) = id.parse::<usize>() else { continue };
        let Some((lab, de)) = rest.split_once(" de ") else { continue };
        let Some((e, m)) = lab.split_once("<=>") else { continue };
        let nums = |s: &str| -> Option<[f64; 3]> {
            let v: Vec<f64> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        if let (Some(expected_lab), Some(measured_lab), Ok(de)) = (nums(e), nums(m), de.trim().parse()) {
            out.push(PatchResult { index, expected_lab, measured_lab, de });
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Good,
    Acceptable,
    Poor,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Good => "Good",
            Verdict::Acceptable => "Acceptable",
            Verdict::Poor => "Poor",
        }
    }
}

/// Limits for the verdict, as (average, maximum) CIEDE2000.
pub const GOOD: (f64, f64) = (1.0, 3.0);
pub const ACCEPTABLE: (f64, f64) = (2.0, 5.0);

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub average: f64,
    pub maximum: f64,
    pub gray_average: f64,
    pub gray_maximum: f64,
    /// Average of the best 90%, which shows whether a few patches dominate
    pub best90_average: f64,
    pub verdict: Verdict,
}

/// Summarize results. `gray` says which patch indices (1 based) are grays.
pub fn summarize(results: &[PatchResult], gray: impl Fn(usize) -> bool) -> Option<Summary> {
    if results.is_empty() {
        return None;
    }
    let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let max = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
    let all: Vec<f64> = results.iter().map(|r| r.de).collect();
    let grays: Vec<f64> = results.iter().filter(|r| gray(r.index)).map(|r| r.de).collect();
    let mut sorted = all.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let best90 = &sorted[..((sorted.len() * 9 + 9) / 10).min(sorted.len())];
    let (average, maximum) = (avg(&all), max(&all));
    let verdict = if average <= GOOD.0 && maximum <= GOOD.1 {
        Verdict::Good
    } else if average <= ACCEPTABLE.0 && maximum <= ACCEPTABLE.1 {
        Verdict::Acceptable
    } else {
        Verdict::Poor
    };
    Some(Summary {
        average,
        maximum,
        gray_average: avg(&grays),
        gray_maximum: max(&grays),
        best90_average: avg(best90),
        verdict,
    })
}

/// Approximate sRGB (0-255) for showing a patch: its device values, which on
/// a display calibrated near sRGB look about right.
pub fn swatch(p: &Patch) -> [u8; 3] {
    p.rgb.map(|v| (v * 2.55).round().clamp(0.0, 255.0) as u8)
}

/// A plain text report.
pub fn report(profile: &Path, summary: &Summary, patches: &[Patch], results: &[PatchResult], correction: Option<&Path>) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "DisplayCraft verification report");
    let _ = writeln!(s, "Date: {}", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let _ = writeln!(s, "Profile: {}", profile.display());
    if let Some(c) = correction {
        let _ = writeln!(s, "Correction: {}", c.display());
    }
    let _ = writeln!(s, "\nVerdict: {}  (Good: average <= {} and maximum <= {}; Acceptable: <= {} and <= {})",
        summary.verdict.label(), GOOD.0, GOOD.1, ACCEPTABLE.0, ACCEPTABLE.1);
    let _ = writeln!(s, "CIEDE2000, readings normalised to white:");
    let _ = writeln!(s, "  Average {:.2}, maximum {:.2}, best 90% average {:.2}", summary.average, summary.maximum, summary.best90_average);
    let _ = writeln!(s, "  Grayscale average {:.2}, maximum {:.2}", summary.gray_average, summary.gray_maximum);
    let _ = writeln!(s, "\n{:>3}  {:<30} {:>18}  {:>20}  {:>20}  {:>6}", "#", "Patch", "Device RGB %", "Expected L*a*b*", "Measured L*a*b*", "dE00");
    for r in results {
        let Some(p) = patches.get(r.index - 1) else { continue };
        let lab = |l: [f64; 3]| format!("{:6.1} {:6.1} {:6.1}", l[0], l[1], l[2]);
        let _ = writeln!(s, "{:>3}  {:<30} {:5.1} {:5.1} {:5.1}  {}  {}  {:6.2}",
            r.index, p.name, p.rgb[0], p.rgb[1], p.rgb[2], lab(r.expected_lab), lab(r.measured_lab), r.de);
    }
    s
}

/// The per-patch results as CSV, one row per patch.
pub fn csv(patches: &[Patch], results: &[PatchResult]) -> String {
    let mut s = String::from("Index,Patch,R %,G %,B %,Expected L*,Expected a*,Expected b*,Measured L*,Measured a*,Measured b*,dE00\n");
    for r in results {
        let Some(p) = patches.get(r.index - 1) else { continue };
        let [e, m] = [r.expected_lab, r.measured_lab];
        let _ = writeln!(s, "{},\"{}\",{:.2},{:.2},{:.2},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
            r.index, p.name.replace('"', "\"\""), p.rgb[0], p.rgb[1], p.rgb[2], e[0], e[1], e[2], m[0], m[1], m[2], r.de);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_set_and_ti1() {
        let p = patches();
        assert_eq!(p.len(), 21 + 12 + 24);
        assert_eq!(p[20].rgb, [100.0, 100.0, 100.0]);
        // Under target/, not the system temporary directory
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/test-tmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("v.ti1");
        write_ti1(&f, &p).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("NUMBER_OF_SETS 57") && text.contains("\n57 "));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_colverify_and_summarizes() {
        let text = "No of test patches = 3\nFile 0 Chose patch 3 as white\n\
            1: 0.453571 0.1 0.2 <=> 0.5 0.1 0.2  de 0.45\n\
            2: 50.0 1.0 -1.0 <=> 50.5 3.0 -2.0  de 3.50\n\
            3: 100.000000 0.000000 0.000000 <=> 100.000000 0.000000 0.000021  de 0.000021\n\
            Verify results:\n  Total errors (CIEDE2000):     peak = 2.5, avg = 1.0\n";
        let r = parse_colverify(text);
        assert_eq!(r.len(), 3);
        assert_eq!(r[1].measured_lab, [50.5, 3.0, -2.0]);
        let s = summarize(&r, |i| i != 2).unwrap();
        // Average 1.32, maximum 3.5: over the Good limits, within Acceptable
        assert!((s.maximum - 3.5).abs() < 1e-9);
        assert!((s.gray_maximum - 0.45).abs() < 1e-9);
        assert_eq!(s.verdict, Verdict::Acceptable);

        let text = csv(&patches(), &r);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[2], "2,\"Gray 5%\",5.00,5.00,5.00,50.000,1.000,-1.000,50.500,3.000,-2.000,3.500");
    }
}
