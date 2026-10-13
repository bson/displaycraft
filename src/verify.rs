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

/// How far the display's channels are from adding up: the largest
/// difference, over white, yellow, cyan and magenta at full drive and each
/// of X, Y and Z, between the patch and the sum of its channels measured
/// alone (less the black every reading includes), as a percentage of
/// white's Y. A shaper+matrix profile assumes they add up exactly.
/// `measured` is in `patches()` order.
pub fn additivity(measured: &[[f64; 3]]) -> Option<f64> {
    let p = patches();
    let at = |name: &str| p.iter().position(|q| q.name == name).and_then(|i| measured.get(i)).copied();
    let (k, w) = (at("Gray 0%")?, at("Gray 100%")?);
    let (r, g, b) = (at("Red 100%")?, at("Green 100%")?, at("Blue 100%")?);
    let sum = |parts: &[[f64; 3]]| -> [f64; 3] {
        [0, 1, 2].map(|c| parts.iter().map(|p| p[c]).sum::<f64>() - (parts.len() - 1) as f64 * k[c])
    };
    let cases = [
        (w, sum(&[r, g, b])),
        (at("Yellow 100%")?, sum(&[r, g])),
        (at("Cyan 100%")?, sum(&[g, b])),
        (at("Magenta 100%")?, sum(&[r, b])),
    ];
    if w[1] <= 0.0 {
        return None;
    }
    Some(cases.iter().flat_map(|(m, s)| (0..3).map(move |c| (m[c] - s[c]).abs())).fold(0.0, f64::max) / w[1] * 100.0)
}

/// Above this (percent of white), the channels don't add up well enough for
/// a shaper+matrix profile, and a LUT may describe the display better. This
/// panel (an LCD) measured 0.26; displays that need a LUT miss by several.
pub const ADDITIVITY_LIMIT: f64 = 2.0;

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
pub fn report(
    profile: &Path,
    kind: &str,
    summary: &Summary,
    patches: &[Patch],
    results: &[PatchResult],
    correction: Option<&Path>,
    additivity: Option<f64>,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "DisplayCraft verification report");
    let _ = writeln!(s, "Date: {}", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let _ = writeln!(s, "Profile: {} ({kind})", profile.display());
    if let Some(c) = correction {
        let _ = writeln!(s, "Correction: {}", c.display());
    }
    let _ = writeln!(s, "\nVerdict: {}  (Good: average <= {} and maximum <= {}; Acceptable: <= {} and <= {})",
        summary.verdict.label(), GOOD.0, GOOD.1, ACCEPTABLE.0, ACCEPTABLE.1);
    let _ = writeln!(s, "CIEDE2000, readings normalised to white:");
    let _ = writeln!(s, "  Average {:.2}, maximum {:.2}, best 90% average {:.2}", summary.average, summary.maximum, summary.best90_average);
    let _ = writeln!(s, "  Grayscale average {:.2}, maximum {:.2}", summary.gray_average, summary.gray_maximum);
    if let Some(a) = additivity {
        let _ = writeln!(s, "Channel additivity: white and the secondaries are off by up to {a:.2}% of white from the sum of their channels");
    }
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

/// The same report as HTML, with charts: a self-contained page that prints
/// cleanly
#[allow(clippy::too_many_arguments)]
pub fn html(
    profile: &Path,
    kind: &str,
    summary: &Summary,
    patches: &[Patch],
    results: &[PatchResult],
    correction: Option<&Path>,
    additivity: Option<f64>,
    curves: Option<&crate::profiles::Curves>,
) -> String {
    use crate::charts::{self, Axis, Chart, Series, Style, escape};
    let verdict_color = |v: Verdict| match v {
        Verdict::Good => "#1f9d55",
        Verdict::Acceptable => "#c98a00",
        Verdict::Poor => "#cc3b3b",
    };
    let de_color = |de: f64| -> [u8; 3] {
        if de <= GOOD.1 { [60, 170, 100] } else if de <= ACCEPTABLE.1 { [220, 160, 40] } else { [210, 70, 70] }
    };
    let name = profile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut s = String::new();
    let _ = write!(
        s,
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>Verification: {}</title><style>\n\
         body {{ font-family: sans-serif; margin: 2em auto; max-width: 980px; color: #222; }}\n\
         h1 {{ font-size: 1.5em; margin-bottom: 0.2em; }} .dim {{ color: #777; }}\n\
         .verdict {{ font-size: 1.6em; font-weight: 600; }}\n\
         table {{ border-collapse: collapse; }} td, th {{ padding: 3px 10px; text-align: right; }}\n\
         th {{ color: #666; font-weight: 600; }} td.name, th.name {{ text-align: left; }}\n\
         tr:nth-child(even) td {{ background: #f4f4f4; }}\n\
         .swatch {{ width: 28px; height: 14px; border: 1px solid #999; display: inline-block; }}\n\
         .charts {{ display: flex; flex-wrap: wrap; gap: 16px; margin: 1.5em 0; }}\n\
         @media print {{ body {{ margin: 0; }} .charts {{ break-inside: avoid; }} tr {{ break-inside: avoid; }} }}\n\
         </style></head><body>\n",
        escape(&name)
    );
    let _ = write!(s, "<h1>Verification of {}</h1>\n<p class=\"dim\">{}", escape(&name), chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let _ = write!(s, " &middot; {}", escape(kind));
    let _ = write!(s, " &middot; {}", escape(&profile.display().to_string()));
    if let Some(c) = correction {
        let _ = write!(s, " &middot; correction {}", escape(&c.display().to_string()));
    }
    s.push_str("</p>\n");
    let _ = write!(
        s,
        "<p><span class=\"verdict\" style=\"color:{}\">{}</span> &nbsp; <span class=\"dim\">Good: average &le; {} and maximum &le; {}; Acceptable: &le; {} and &le; {} (CIEDE2000, readings normalised to white)</span></p>\n",
        verdict_color(summary.verdict), summary.verdict.label(), GOOD.0, GOOD.1, ACCEPTABLE.0, ACCEPTABLE.1
    );
    let _ = write!(
        s,
        "<table><tr><th class=\"name\">Average</th><td>{:.2}</td><th class=\"name\">Maximum</th><td>{:.2}</td><th class=\"name\">Best 90% average</th><td>{:.2}</td></tr>\n\
         <tr><th class=\"name\">Grayscale average</th><td>{:.2}</td><th class=\"name\">Grayscale maximum</th><td>{:.2}</td>",
        summary.average, summary.maximum, summary.best90_average, summary.gray_average, summary.gray_maximum
    );
    if let Some(a) = additivity {
        let _ = write!(s, "<th class=\"name\">Channel additivity</th><td>off by {a:.2}%</td>");
    }
    s.push_str("</tr></table>\n<div class=\"charts\">\n");

    // ΔE per patch
    let max = results.iter().map(|r| r.de).fold(ACCEPTABLE.1, f64::max).ceil();
    let mut bars = Series::new("", [0, 0, 0], results.iter().map(|r| (r.index as f64, r.de)).collect(), Style::Bars);
    bars.bar_colors = Some(results.iter().map(|r| de_color(r.de)).collect());
    let limits = |y: f64, c: [u8; 3]| Series::new("", c, vec![(0.5, y), (results.len() as f64 + 0.5, y)], Style::Line { width: 1.0, dashed: true });
    let de_chart = Chart {
        title: "ΔE00 per patch".into(),
        x: Axis::new("patch", 0.0, ((results.len() as f64 / 10.0).ceil() * 10.0).max(10.0), 10.0),
        y: Axis::new("", 0.0, max, if max > 10.0 { 2.0 } else { 1.0 }),
        series: vec![bars, limits(GOOD.1, [60, 170, 100]), limits(ACCEPTABLE.1, [220, 160, 40])],
        equal_scale: false,
    };
    s.push_str(&de_chart.svg(560.0, 260.0));

    // Gray balance: a* and b* of the measured gray ramp (the ColorChecker
    // neutrals come later in the set, so would run the line backwards)
    let grays: Vec<(f64, [f64; 3])> = results
        .iter()
        .filter_map(|r| {
            let p = patches.get(r.index - 1)?;
            p.name.starts_with("Gray ").then_some((p.rgb[0], r.measured_lab))
        })
        .collect();
    if !grays.is_empty() {
        let lim = grays.iter().flat_map(|(_, l)| [l[1].abs(), l[2].abs()]).fold(2.0, f64::max).ceil();
        let gray_chart = Chart {
            title: "Gray balance (measured)".into(),
            x: Axis::new("gray level %", 0.0, 100.0, 20.0),
            y: Axis::new("", -lim, lim, if lim > 4.0 { 2.0 } else { 1.0 }),
            series: vec![
                Series::new("", [150, 150, 150], vec![(0.0, 0.0), (100.0, 0.0)], Style::Line { width: 1.0, dashed: true }),
                Series::new("a* (+red, -green)", [210, 70, 90], grays.iter().map(|(v, l)| (*v, l[1])).collect(), Style::Line { width: 1.6, dashed: false }),
                Series::new("b* (+yellow, -blue)", [70, 110, 220], grays.iter().map(|(v, l)| (*v, l[2])).collect(), Style::Line { width: 1.6, dashed: false }),
            ],
            equal_scale: false,
        };
        s.push_str(&gray_chart.svg(380.0, 260.0));
    }
    if let Some(c) = curves {
        if let Some(p) = c.primaries {
            s.push_str(&charts::gamut(p, c.white).svg(320.0, 340.0));
        }
        if let Some(cal) = &c.calibration {
            s.push_str(&charts::calibration(cal).svg(300.0, 300.0));
        }
        if let Some(t) = &c.tone {
            s.push_str(&charts::tone(t).svg(300.0, 300.0));
        }
    }
    s.push_str("</div>\n");

    s.push_str(
        "<table><tr><th>#</th><th></th><th class=\"name\">Patch</th><th>Device RGB %</th><th>Expected L*a*b*</th><th>Measured L*a*b*</th><th>&Delta;E00</th></tr>\n",
    );
    for r in results {
        let Some(p) = patches.get(r.index - 1) else { continue };
        let [cr, cg, cb] = swatch(p);
        // Values that round to zero print without a sign
        let one = |v: f64| if v.abs() < 0.05 { 0.0 } else { v };
        let lab = |l: [f64; 3]| format!("{:.1} {:.1} {:.1}", one(l[0]), one(l[1]), one(l[2]));
        let c = de_color(r.de);
        let _ = writeln!(
            s,
            "<tr><td>{}</td><td><span class=\"swatch\" style=\"background:rgb({cr},{cg},{cb})\"></span></td><td class=\"name\">{}</td>\
             <td>{:.0} {:.0} {:.0}</td><td>{}</td><td>{}</td><td style=\"color:rgb({},{},{});font-weight:600\">{:.2}</td></tr>",
            r.index, escape(&p.name), p.rgb[0], p.rgb[1], p.rgb[2], lab(r.expected_lab), lab(r.measured_lab), c[0], c[1], c[2], r.de
        );
    }
    s.push_str("</table>\n<p class=\"dim\">Made by DisplayCraft with ArgyllCMS (fakeread, dispread, colverify). Swatches show device values, which look about right on a display calibrated near sRGB.</p>\n</body></html>\n");
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
        let page = html(Path::new("/x/p.icc"), "LUT profile", &s, &patches(), &r, None, Some(0.3), None);
        assert!(page.starts_with("<!DOCTYPE html>") && page.contains("<svg") && page.contains("Gray 5%") && page.ends_with("</html>\n"));
        assert!(page.contains("LUT profile"));

        // A display whose channels add up, then one whose white falls 5% short
        let p = patches();
        let channel = |name: &str| match name {
            n if n.starts_with("Red") => [41.0, 21.0, 2.0],
            n if n.starts_with("Green") => [36.0, 72.0, 12.0],
            n if n.starts_with("Blue") => [18.0, 7.0, 95.0],
            _ => [0.0; 3],
        };
        let mut m: Vec<[f64; 3]> = p
            .iter()
            .map(|q| {
                let parts: Vec<[f64; 3]> = match q.name.as_str() {
                    "Gray 100%" => vec![channel("Red"), channel("Green"), channel("Blue")],
                    "Yellow 100%" => vec![channel("Red"), channel("Green")],
                    "Cyan 100%" => vec![channel("Green"), channel("Blue")],
                    "Magenta 100%" => vec![channel("Red"), channel("Blue")],
                    n => vec![channel(n)],
                };
                [0, 1, 2].map(|c| 0.1 + parts.iter().map(|x| x[c]).sum::<f64>())
            })
            .collect();
        assert!(additivity(&m).unwrap() < 1e-9);
        let w = p.iter().position(|q| q.name == "Gray 100%").unwrap();
        m[w] = m[w].map(|v| v * 0.95);
        // Largest miss is in Z: 5% of 109.1, against white's Y of 0.95 * 100.1
        let expected = 0.05 * 109.1 / (0.95 * 100.1) * 100.0;
        assert!((additivity(&m).unwrap() - expected).abs() < 1e-6 && expected > ADDITIVITY_LIMIT);

        let text = csv(&patches(), &r);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[2], "2,\"Gray 5%\",5.00,5.00,5.00,50.000,1.000,-1.000,50.500,3.000,-2.000,3.500");
    }
}
