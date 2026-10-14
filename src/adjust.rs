//! dispcal's interactive adjustment of the display's own controls, read
//! from its output. DisplayCraft runs it on its own from the Parameters
//! page and leaves with Exit (8), so calibrating stays a separate step.
//!
//! Without `-m`, dispcal shows a menu ("Press 1 .. 7") after the
//! instrument is placed. DisplayCraft answers with Check all (5) each time
//! the menu returns, so black, 50% gray and white are read over and over,
//! with the display uncalibrated. With `-v` each check prints:
//!
//! ```text
//! Doing check measurements
//! Black = XYZ 0.2701 0.2860 0.3471
//! Grey  = XYZ 52.701 55.672 60.214
//! White = XYZ 232.92 244.94 263.56
//! 1%    = XYZ  2.457  2.588  2.791
//!   Target Brightness = 245.000, Current = 244.949, error = -0.0%
//!   Target 50% Level  = 55.413, Current = 55.672 (Aprox. Gamma 2.14), error =  0.1%
//!   Target white = x 0.3145, y 0.3299, Current = x 0.3145, y 0.3299, error =  0.12 DE
//! ```
//!
//! dispcal reads black, gray and white as one batch, so all three arrive
//! together. The 50% target comes from the black and white just measured,
//! through the tone curve calibration will aim for, so it moves as they're
//! adjusted. With a native white there's no "Target white", only "Current
//! white", and the target is the display's own white.

/// The three points read
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Point {
    Black,
    Mid,
    White,
}

impl Point {
    pub const ALL: [Point; 3] = [Point::Black, Point::Mid, Point::White];

    pub fn label(self) -> &'static str {
        match self {
            Point::Black => "Black",
            Point::Mid => "50% gray",
            Point::White => "White",
        }
    }
}

/// What dispcal is doing in its adjustment stage, and the latest readings
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The menu has been shown: past the placement prompt, adjusting
    pub started: bool,
    /// Waiting at the menu, for the next check (or Exit)
    pub menu: bool,
    /// A check is under way
    pub checking: bool,
    /// Done was asked for: Exit at the next menu, since a key pressed while
    /// reading would interrupt the reading
    pub stopping: bool,
    /// XYZ in cd/m², kept from one check to the next
    pub black: Option<[f64; 3]>,
    pub mid: Option<[f64; 3]>,
    pub white: Option<[f64; 3]>,
    /// The 50% level to aim for, from the last black and white, cd/m²
    pub target_mid: Option<f64>,
    /// The white point to aim for; None for the display's own (native)
    pub target_xy: Option<(f64, f64)>,
}

/// "0.2701 0.2860 0.3471" -> the three numbers
fn xyz(rest: &str) -> Option<[f64; 3]> {
    let n: Vec<f64> = rest.split_whitespace().filter_map(|w| w.parse().ok()).collect();
    (n.len() == 3).then(|| [n[0], n[1], n[2]])
}

impl State {
    /// Take in a complete line of dispcal's output
    pub fn line(&mut self, line: &str) {
        let line = line.trim();
        if line.starts_with("Press 1 .. 7") {
            self.started = true;
            self.menu = true;
            self.checking = false;
        } else if line.starts_with("Doing check measurements") {
            self.menu = false;
            self.checking = true;
        } else if let Some(rest) = line.strip_prefix("Black = XYZ") {
            self.black = xyz(rest).or(self.black);
        } else if let Some(rest) = line.strip_prefix("Grey  = XYZ").or_else(|| line.strip_prefix("Grey = XYZ")) {
            self.mid = xyz(rest).or(self.mid);
        } else if let Some(rest) = line.strip_prefix("White = XYZ") {
            self.white = xyz(rest).or(self.white);
        } else if let Some(rest) = line.strip_prefix("Target 50% Level") {
            // "= 55.413, Current = 55.672 (Aprox. Gamma 2.14), error =  0.1%"
            let value = rest.trim_start_matches([' ', '=']).split(',').next().and_then(|v| v.trim().parse().ok());
            self.target_mid = value.or(self.target_mid);
        } else if let Some(rest) = line.strip_prefix("Target white = x") {
            // "0.3145, y 0.3299, Current = ..."
            let mut parts = rest.split(',');
            let x = parts.next().and_then(|v| v.trim().parse().ok());
            let y = parts.next().and_then(|v| v.trim().strip_prefix('y')?.trim().parse().ok());
            if let (Some(x), Some(y)) = (x, y) {
                self.target_xy = Some((x, y));
            }
        }
    }

    /// Whether dispcal is past the placement prompt and in its adjustment
    /// stage
    pub fn active(&self) -> bool {
        self.started
    }

    pub fn reading(&self, p: Point) -> Option<[f64; 3]> {
        match p {
            Point::Black => self.black,
            Point::Mid => self.mid,
            Point::White => self.white,
        }
    }

    /// The white point deviations are measured from: the target, or for a
    /// native one the display's own white
    pub fn reference_xy(&self) -> Option<(f64, f64)> {
        self.target_xy.or_else(|| self.white.map(xy))
    }
}

pub fn xy(c: [f64; 3]) -> (f64, f64) {
    let s = c[0] + c[1] + c[2];
    if s <= 0.0 { (0.0, 0.0) } else { (c[0] / s, c[1] / s) }
}

/// How far a reading's color is from the target white
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deviation {
    /// CIEDE2000 between the two at the same luminance, as dispcal works it
    /// out: each made Y = 1, Lab relative to the target
    pub de: f64,
    /// The reading's correlated color temperature, K
    pub cct: f64,
    /// It less the target's, K
    pub cct_diff: f64,
}

pub fn deviation(reading: [f64; 3], target: (f64, f64)) -> Option<Deviation> {
    if reading[1] <= 0.0 || target.1 <= 0.0 {
        return None;
    }
    let white = [target.0 / target.1, 1.0, (1.0 - target.0 - target.1) / target.1];
    let c = reading.map(|v| v / reading[1]);
    let de = de2000(lab(white, white), lab(c, white));
    let cct = crate::report::cct(xy(reading));
    Some(Deviation { de, cct, cct_diff: cct - crate::report::cct(target) })
}

/// CIE Lab of `c` relative to `white`, both XYZ
fn lab(c: [f64; 3], white: [f64; 3]) -> [f64; 3] {
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let [fx, fy, fz] = [0, 1, 2].map(|i| f(c[i] / white[i]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000, as formulated by Sharma, Wu and Dalal
pub fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    let rad = f64::to_radians;
    let c7 = |c: f64| (c.powi(7) / (c.powi(7) + 25f64.powi(7))).sqrt();
    let cm = (a[1].hypot(a[2]) + b[1].hypot(b[2])) / 2.0;
    let g = 0.5 * (1.0 - c7(cm));
    let (a1, a2) = (a[1] * (1.0 + g), b[1] * (1.0 + g));
    let (c1, c2) = (a1.hypot(a[2]), a2.hypot(b[2]));
    let hue = |a: f64, b: f64| if a == 0.0 && b == 0.0 { 0.0 } else { b.atan2(a).to_degrees().rem_euclid(360.0) };
    let (h1, h2) = (hue(a1, a[2]), hue(a2, b[2]));
    let dh = if c1 * c2 == 0.0 {
        0.0
    } else if (h2 - h1).abs() <= 180.0 {
        h2 - h1
    } else if h2 > h1 {
        h2 - h1 - 360.0
    } else {
        h2 - h1 + 360.0
    };
    let dl = b[0] - a[0];
    let dc = c2 - c1;
    let dhh = 2.0 * (c1 * c2).sqrt() * rad(dh / 2.0).sin();
    let lm = (a[0] + b[0]) / 2.0;
    let cm = (c1 + c2) / 2.0;
    let hm = if c1 * c2 == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * rad(hm - 30.0).cos() + 0.24 * rad(2.0 * hm).cos() + 0.32 * rad(3.0 * hm + 6.0).cos()
        - 0.20 * rad(4.0 * hm - 63.0).cos();
    let dtheta = 30.0 * (-((hm - 275.0) / 25.0).powi(2)).exp();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cm;
    let sh = 1.0 + 0.015 * cm * t;
    let rt = -rad(2.0 * dtheta).sin() * 2.0 * c7(cm);
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn de2000_matches_sharmas_test_data() {
        // Pairs 1, 7, 17 and 25 of Sharma, Wu and Dalal's table
        let pairs = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [56.0, -27.0, -3.0], 31.9030),
            ([60.2574, -34.0099, 36.2677], [60.4626, -34.1751, 39.4387], 1.2644),
        ];
        for (a, b, want) in pairs {
            assert!((de2000(a, b) - want).abs() < 1e-4, "{a:?} {b:?}: {} not {want}", de2000(a, b));
            assert!((de2000(b, a) - want).abs() < 1e-4);
        }
    }

    #[test]
    fn deviation_from_the_target_white() {
        let d65 = (0.3127, 0.3290);
        let on = deviation([95.05, 100.0, 108.9], d65).unwrap();
        assert!(on.de < 0.05 && on.cct_diff.abs() < 15.0, "{on:?}");
        // A warmer gray, at a tenth the luminance
        let warm = deviation([9.7, 10.0, 9.6], d65).unwrap();
        assert!(warm.de > 3.0 && warm.cct_diff < -300.0, "{warm:?}");
    }

    #[test]
    fn follows_the_checks() {
        let mut s = State::default();
        s.line("Target white = 6500.000000 degrees kelvin Daylight spectrum");
        assert!(!s.active());
        s.line("Press 1 .. 7");
        assert!(s.menu && s.active());
        s.line("Doing check measurements");
        assert!(s.checking && !s.menu);
        s.line("Black = XYZ 0.2701 0.2860 0.3471");
        s.line("Grey  = XYZ 52.701 55.672 60.214");
        s.line("White = XYZ 232.92 244.94 263.56");
        s.line("1%    = XYZ  2.457  2.588  2.791");
        s.line("  Target Brightness = 245.000, Current = 244.949, error = -0.0%");
        s.line("  Target 50% Level  = 55.413, Current = 55.672 (Aprox. Gamma 2.14), error =  0.1%");
        s.line("  Target white = x 0.3145, y 0.3299, Current = x 0.3145, y 0.3299, error =  0.12 DE");
        s.line("  Target black = x 0.3145, y 0.3299, Current = x 0.3077, y 0.3166, error =  4.20 DE");
        assert_eq!(s.black, Some([0.2701, 0.2860, 0.3471]));
        assert_eq!(s.mid.map(|c| c[1]), Some(55.672));
        assert_eq!(s.white.map(|c| c[1]), Some(244.94));
        assert_eq!((s.target_mid, s.target_xy), (Some(55.413), Some((0.3145, 0.3299))));
        s.line("Press 1 .. 7");
        assert!(s.menu && !s.checking);
        // Readings stay until the next ones arrive
        s.line("Doing check measurements");
        assert_eq!(s.white.map(|c| c[1]), Some(244.94));
    }

    #[test]
    fn native_white_is_the_displays_own() {
        let mut s = State::default();
        s.line("White = XYZ 232.92 244.94 263.56");
        s.line("  Current white = x 0.3142, y 0.3304, CDT 6400K DE 2K  1.2");
        assert_eq!(s.target_xy, None);
        let (x, y) = s.reference_xy().unwrap();
        assert!((x - 0.31415).abs() < 1e-4 && (y - 0.33036).abs() < 1e-4, "{x} {y}");
    }
}
