//! Display report: what the display does, from a few measured patches.
//!
//! `dispread` measures black, white, a gray ramp and the three primaries;
//! everything else is worked out here. Coverage of the standard color
//! spaces is by area in the CIE 1976 u'v' diagram, as display makers quote
//! it, so it says nothing about how the gamut extends in lightness.

use std::fmt::Write as _;
use std::path::Path;

/// The patches, as device RGB in percent. Indices below rely on the order.
pub const PATCHES: [[f64; 3]; 16] = [
    [0.0, 0.0, 0.0],
    [100.0, 100.0, 100.0],
    [10.0, 10.0, 10.0],
    [20.0, 20.0, 20.0],
    [30.0, 30.0, 30.0],
    [40.0, 40.0, 40.0],
    [50.0, 50.0, 50.0],
    [60.0, 60.0, 60.0],
    [70.0, 70.0, 70.0],
    [80.0, 80.0, 80.0],
    [90.0, 90.0, 90.0],
    [100.0, 0.0, 0.0],
    [0.0, 100.0, 0.0],
    [0.0, 0.0, 100.0],
    [0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0],
];
/// Black is measured three times, spread through the set, and the median
/// used: it's the reading most easily thrown off (by the instrument
/// shifting, or light getting in), and it sets the contrast ratio
const BLACKS: [usize; 3] = [0, 14, 15];
const WHITE: usize = 1;
const GRAYS: std::ops::Range<usize> = 2..11;
const GRAY50: usize = 6;
const PRIMARIES: std::ops::Range<usize> = 11..14;

/// Reference gamuts: name and red, green, blue primaries in CIE xy
pub const REFERENCES: [(&str, [(f64, f64); 3]); 3] = [
    ("sRGB", [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)]),
    ("Display P3", [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)]),
    ("Adobe RGB", [(0.640, 0.330), (0.210, 0.710), (0.150, 0.060)]),
];

pub fn write_ti1(path: &Path) -> std::io::Result<()> {
    let mut s = String::from(
        "CTI1\n\nDESCRIPTOR \"DisplayCraft display report patches\"\nORIGINATOR \"DisplayCraft\"\nCOLOR_REP \"RGB\"\n\n\
         NUMBER_OF_FIELDS 7\nBEGIN_DATA_FORMAT\nSAMPLE_ID RGB_R RGB_G RGB_B XYZ_X XYZ_Y XYZ_Z\nEND_DATA_FORMAT\n\n",
    );
    let _ = writeln!(s, "NUMBER_OF_SETS {}\nBEGIN_DATA", PATCHES.len());
    for (i, p) in PATCHES.iter().enumerate() {
        let _ = writeln!(s, "{} {:.4} {:.4} {:.4} 0 0 0", i + 1, p[0], p[1], p[2]);
    }
    s.push_str("END_DATA\n");
    std::fs::write(path, s)
}

/// A calibration that changes nothing, for measuring the display's own
/// response (`dispread -k`)
pub fn write_linear_cal(path: &Path) -> std::io::Result<()> {
    let mut s = String::from(
        "CAL\n\nDESCRIPTOR \"Argyll Device Calibration Curves\"\nORIGINATOR \"DisplayCraft\"\n\
         DEVICE_CLASS \"DISPLAY\"\nCOLOR_REP \"RGB\"\n\nNUMBER_OF_FIELDS 4\nBEGIN_DATA_FORMAT\nRGB_I RGB_R RGB_G RGB_B\n\
         END_DATA_FORMAT\n\nNUMBER_OF_SETS 256\nBEGIN_DATA\n",
    );
    for i in 0..256 {
        let v = i as f64 / 255.0;
        let _ = writeln!(s, "{v:.6} {v:.6} {v:.6} {v:.6}");
    }
    s.push_str("END_DATA\n");
    std::fs::write(path, s)
}

/// Absolute XYZ (cd/m²) of each patch, in `PATCHES` order, from dispread's
/// .ti3. Its XYZ is scaled so white is Y = 100; LUMINANCE_XYZ_CDM2 gives
/// white's absolute value.
pub fn read_ti3(text: &str) -> Option<Vec<[f64; 3]>> {
    read_xyz(text, PATCHES.len(), true)
}

/// XYZ of patches 1..=`count` from a dispread .ti3, in cd/m² if `absolute`,
/// otherwise as recorded (white Y = 100)
pub fn read_xyz(text: &str, count: usize, absolute: bool) -> Option<Vec<[f64; 3]>> {
    let white_y: f64 = text
        .lines()
        .find_map(|l| l.strip_prefix("LUMINANCE_XYZ_CDM2"))
        .and_then(|v| v.trim().trim_matches('"').split_whitespace().nth(1)?.parse().ok())
        .or((!absolute).then_some(100.0))?;
    let fields: Vec<&str> = text
        .split("BEGIN_DATA_FORMAT")
        .nth(1)?
        .split("END_DATA_FORMAT")
        .next()?
        .split_whitespace()
        .collect();
    let col = |name: &str| fields.iter().position(|f| *f == name);
    let (id, x, y, z) = (col("SAMPLE_ID")?, col("XYZ_X")?, col("XYZ_Y")?, col("XYZ_Z")?);
    let data = text.split("\nBEGIN_DATA\n").nth(1)?.split("END_DATA").next()?;
    let mut out = vec![None; count];
    for line in data.lines() {
        let v: Vec<&str> = line.split_whitespace().collect();
        let Some(i) = v.get(id).and_then(|s| s.parse::<usize>().ok()).filter(|i| (1..=count).contains(i)) else { continue };
        let num = |c: usize| v.get(c)?.parse::<f64>().ok().map(|n| n * white_y / 100.0);
        out[i - 1] = Some([num(x)?, num(y)?, num(z)?]);
    }
    out.into_iter().collect()
}

fn xy(c: [f64; 3]) -> (f64, f64) {
    let s = c[0] + c[1] + c[2];
    if s <= 0.0 { (0.0, 0.0) } else { (c[0] / s, c[1] / s) }
}

fn uv((x, y): (f64, f64)) -> (f64, f64) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 9.0 * y / d)
}

/// McCamy's approximation of correlated color temperature
pub fn cct((x, y): (f64, f64)) -> f64 {
    let n = (x - 0.3320) / (0.1858 - y);
    449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33
}

fn area(poly: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        a.0 * b.1 - b.0 * a.1
    })
    .sum::<f64>()
    .abs()
        / 2.0
}

/// Make a triangle counter-clockwise, as `clip` expects of its clip polygon
fn ccw(mut t: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let signed: f64 = (0..3).map(|i| t[i].0 * t[(i + 1) % 3].1 - t[(i + 1) % 3].0 * t[i].1).sum();
    if signed < 0.0 {
        t.reverse();
    }
    t
}

/// Sutherland-Hodgman: `subject` clipped to the convex, counter-clockwise
/// polygon `clip`
fn clip(subject: &[(f64, f64)], clip: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = subject.to_vec();
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let inside = |p: (f64, f64)| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0) >= 0.0;
        let cross = |p: (f64, f64), q: (f64, f64)| {
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
            let t = (ex * (p.1 - a.1) - ey * (p.0 - a.0)) / (ey * dx - ex * dy);
            (p.0 + t * dx, p.1 + t * dy)
        };
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            match (inside(p), inside(q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(cross(p, q)),
                (false, true) => {
                    out.push(cross(p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    pub name: &'static str,
    /// Share of the reference's area the display covers, percent
    pub covered: f64,
    /// The display's area relative to the reference's, percent
    pub relative: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub black: f64,
    pub white: f64,
    pub contrast: f64,
    /// Least-squares power law over the gray ramp, black removed
    pub gamma: f64,
    /// 50% gray: with white, what sets how bright a display looks
    pub gray50: f64,
    pub white_xy: (f64, f64),
    pub cct: f64,
    /// Red, green and blue in CIE xy
    pub primaries: [(f64, f64); 3],
    pub coverage: Vec<Coverage>,
}

pub fn analyze(m: &[[f64; 3]]) -> Option<Report> {
    if m.len() != PATCHES.len() {
        return None;
    }
    let mut blacks = BLACKS.map(|i| m[i]);
    blacks.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let k = blacks[1];
    let (black, white) = (k[1], m[WHITE][1]);
    if white <= black {
        return None;
    }
    // Fit log(Y) = gamma log(v) through the origin
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for i in GRAYS {
        let y = (m[i][1] - black) / (white - black);
        if y > 0.0 {
            let lv = (PATCHES[i][0] / 100.0).ln();
            sxy += lv * y.ln();
            sxx += lv * lv;
        }
    }
    // The panel's black glow is in every reading; left in, it pulls the
    // primaries toward white
    let primaries: Vec<(f64, f64)> = PRIMARIES.map(|i| xy([0, 1, 2].map(|c| m[i][c] - k[c]))).collect();
    let display = ccw(primaries.iter().map(|&p| uv(p)).collect());
    let coverage = REFERENCES
        .iter()
        .map(|(name, prims)| {
            let reference = ccw(prims.iter().map(|&p| uv(p)).collect());
            let r = area(&reference);
            Coverage { name, covered: 100.0 * area(&clip(&display, &reference)) / r, relative: 100.0 * area(&display) / r }
        })
        .collect();
    let white_xy = xy(m[WHITE]);
    Some(Report {
        black,
        white,
        contrast: if black > 0.0 { white / black } else { f64::INFINITY },
        gamma: if sxx > 0.0 { sxy / sxx } else { f64::NAN },
        gray50: m[GRAY50][1],
        white_xy,
        cct: cct(white_xy),
        primaries: [primaries[0], primaries[1], primaries[2]],
        coverage,
    })
}

/// A plain text version, for saving
pub fn text(r: &Report, title: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "DisplayCraft display report: {title}");
    let _ = writeln!(s, "Date: {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let _ = writeln!(s, "White luminance   {:.1} cd/m²", r.white);
    let _ = writeln!(s, "Black luminance   {:.3} cd/m²", r.black);
    let _ = writeln!(s, "Contrast ratio    {:.0}:1", r.contrast);
    let _ = writeln!(s, "Gamma             {:.2}", r.gamma);
    let _ = writeln!(s, "50% gray          {:.2} cd/m² ({:.1}% of white)", r.gray50, 100.0 * r.gray50 / r.white);
    let _ = writeln!(s, "White point       x {:.4}, y {:.4} (about {:.0} K)", r.white_xy.0, r.white_xy.1, r.cct);
    for (name, p) in ["Red", "Green", "Blue"].iter().zip(r.primaries) {
        let _ = writeln!(s, "{name:<17} x {:.4}, y {:.4}", p.0, p.1);
    }
    let _ = writeln!(s, "\nGamut, by area in CIE 1976 u'v':");
    for c in &r.coverage {
        let _ = writeln!(s, "  {:<11} {:5.1}% covered, {:5.1}% relative size", c.name, c.covered, c.relative);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xyz_from_xy(x: f64, y: f64, lum: f64) -> [f64; 3] {
        [x / y * lum, lum, (1.0 - x - y) / y * lum]
    }

    #[test]
    fn analyzes_an_srgb_display() {
        // A display with sRGB primaries, D65 white, gamma 2.2, 200 cd/m²
        let mut m = Vec::new();
        for p in PATCHES {
            let lum = |v: f64| (v / 100.0).powf(2.2);
            let (r, g, b) = (lum(p[0]), lum(p[1]), lum(p[2]));
            let y = 199.8 * (0.2126 * r + 0.7152 * g + 0.0722 * b);
            let c = if r > 0.0 && g == 0.0 { xyz_from_xy(0.64, 0.33, y) }
                else if g > 0.0 && r == 0.0 { xyz_from_xy(0.30, 0.60, y) }
                else if b > 0.0 && r == 0.0 { xyz_from_xy(0.15, 0.06, y) }
                else { xyz_from_xy(0.3127, 0.3290, y) };
            // Black (0.2 cd/m², white in color) adds to every patch, as on
            // a real panel
            let k = xyz_from_xy(0.3127, 0.3290, 0.2);
            m.push([c[0] + k[0], c[1] + k[1], c[2] + k[2]]);
        }
        let r = analyze(&m).unwrap();
        assert!((r.gamma - 2.2).abs() < 0.02, "{}", r.gamma);
        assert!((r.contrast - 1000.0).abs() < 1.0);
        assert!((r.gray50 - (199.8 * 0.5f64.powf(2.2) + 0.2)).abs() < 1e-9, "{}", r.gray50);
        assert!((r.cct - 6504.0).abs() < 30.0, "{}", r.cct);
        let srgb = &r.coverage[0];
        assert!((srgb.covered - 100.0).abs() < 0.01 && (srgb.relative - 100.0).abs() < 0.01);
        let p3 = &r.coverage[1];
        assert!(p3.covered < 80.0 && (p3.covered - p3.relative).abs() < 0.01, "{p3:?}");
    }

    #[test]
    fn reads_dispread_output() {
        let text = "CTI3\nLUMINANCE_XYZ_CDM2 \"190.0 200.0 210.0\"\nNUMBER_OF_FIELDS 7\nBEGIN_DATA_FORMAT\n\
                    SAMPLE_ID RGB_R RGB_G RGB_B XYZ_X XYZ_Y XYZ_Z\nEND_DATA_FORMAT\nNUMBER_OF_SETS 16\nBEGIN_DATA\n"
            .to_string()
            + &(1..=16).map(|i| format!("{i} 0 0 0 {}.0 50.0 1.0\n", i)).collect::<String>()
            + "END_DATA\n";
        let m = read_ti3(&text).unwrap();
        assert_eq!(m.len(), 16);
        assert_eq!(m[1], [4.0, 100.0, 2.0]);
    }
}
