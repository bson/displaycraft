//! Display references (.dcref): what a display showed when measured, saved
//! so a display, this one after it has drifted or another here or on
//! another computer, can be calibrated to look the same. Made from a
//! report's measurements, saved as a .dcref file, or from a profile's,
//! kept in the profile itself in a private tag (TAG). Either way it's the
//! same TOML text.
//!
//! Matching uses white's chromaticity, its luminance, the 50% gray and the
//! black level the reference was calibrated to. White and 50% gray set how
//! bright a display looks overall, and dispcal's -g aims its curve through
//! the same relative 50% level as the gamma it's given (black included), so
//! the gamma here is the one whose 50% point is the reference's.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{profiles, report};

/// Bumped only for changes older DisplayCraft can't read sensibly
pub const FORMAT: u32 = 1;

/// The profile tag holding a reference. Programs that don't know a tag
/// ignore it, as the ICC specification has them do.
pub const TAG: &[u8; 4] = b"DCrf";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub format: u32,
    /// The display measured, as DisplayCraft names it
    pub display: String,
    /// Its maker and model, where the system says
    pub model: Option<String>,
    /// Local time of the measurement, "2026-10-14 15:30"
    pub measured: String,
    /// "as calibrated now" or "uncalibrated" for a report, "as calibrated"
    /// for a profile's
    pub mode: String,
    /// The profile it was made with, for a profile's
    pub profile: Option<String>,
    pub instrument: String,
    /// The colorimeter correction or display type, if any. Matching is only
    /// as good as each display's own correction.
    pub correction: Option<String>,
    /// White's luminance, cd/m²
    pub white: f64,
    pub white_x: f64,
    pub white_y: f64,
    /// Correlated color temperature of white, K
    pub cct: f64,
    pub black: f64,
    /// The black level the reference's calibration aimed for, cd/m², or 0
    /// for device black. None where that isn't known: measured
    /// uncalibrated, or calibrated other than by DisplayCraft.
    pub black_target: Option<f64>,
    pub gray50: f64,
    /// The gamma whose 50% point is the reference's, for dispcal's -g
    pub gamma50: f64,
    /// Red, green and blue in CIE xy, black removed; None where the
    /// measurement had no pure primaries (targen doesn't promise them)
    pub primaries: Option<[[f64; 2]; 3]>,
    /// Every patch measured: device RGB in percent and XYZ in cd/m²
    pub patches: Vec<Patch>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub rgb: [f64; 3],
    pub xyz: [f64; 3],
}

/// What's known about the measurement besides the readings
pub struct About {
    pub display: String,
    pub model: Option<String>,
    pub measured: String,
    pub mode: String,
    pub profile: Option<String>,
    pub instrument: String,
    pub correction: Option<String>,
    pub black_target: Option<f64>,
}

/// The maker and model in an ArgyllCMS display name, "eDP-1 (Sharp
/// Corporation eDP-1)" -> "Sharp Corporation eDP-1". Names without it
/// (macOS's "Built-in Display") give None.
pub fn model(display_name: &str) -> Option<String> {
    let (_, rest) = display_name.split_once(" (")?;
    let model = rest.strip_suffix(')')?.trim();
    (!model.is_empty()).then(|| model.to_string())
}

/// The black target recorded in a dispcal .cal: its TARGET_BLACK_BRIGHTNESS,
/// or 0 (device black) where dispcal wrote none
pub fn cal_black_target(cal: &str) -> f64 {
    cal.lines()
        .find_map(|l| l.strip_prefix("TARGET_BLACK_BRIGHTNESS"))
        .and_then(|v| v.trim().trim_matches('"').parse().ok())
        .unwrap_or(0.0)
}

/// Every patch of a dispread .ti3, XYZ made absolute with
/// LUMINANCE_XYZ_CDM2. Only the first table: a .ti3 measured through a
/// calibration carries the .cal as a second.
pub fn read_ti3(text: &str) -> Option<Vec<Patch>> {
    let white_y: f64 = text
        .lines()
        .find_map(|l| l.strip_prefix("LUMINANCE_XYZ_CDM2"))
        .and_then(|v| v.trim().trim_matches('"').split_whitespace().nth(1)?.parse().ok())?;
    let fields: Vec<&str> = text.split("BEGIN_DATA_FORMAT").nth(1)?.split("END_DATA_FORMAT").next()?.split_whitespace().collect();
    let col = |name: &str| fields.iter().position(|f| *f == name);
    let cols = [col("RGB_R")?, col("RGB_G")?, col("RGB_B")?, col("XYZ_X")?, col("XYZ_Y")?, col("XYZ_Z")?];
    let data = text.split("\nBEGIN_DATA\n").nth(1)?.split("END_DATA").next()?;
    data.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let v: Vec<&str> = line.split_whitespace().collect();
            let n: Vec<f64> = cols.iter().map(|&c| v.get(c)?.parse().ok()).collect::<Option<_>>()?;
            Some(Patch { rgb: [n[0], n[1], n[2]], xyz: [n[3], n[4], n[5]].map(|c| c * white_y / 100.0) })
        })
        .collect()
}

/// Relative 50% level -> the gamma dispcal's -g needs to reproduce it
pub fn gamma50(gray50: f64, white: f64) -> f64 {
    (gray50 / white).ln() / 0.5f64.ln()
}

fn xy(c: [f64; 3]) -> [f64; 2] {
    let s = c[0] + c[1] + c[2];
    if s <= 0.0 { [0.0, 0.0] } else { [c[0] / s, c[1] / s] }
}

/// The median reading, by luminance, of the patches with this RGB. Repeats
/// of white and black are spread through a measurement so that one thrown
/// off (by drift, or light getting in) doesn't count.
fn median(patches: &[Patch], rgb: [f64; 3]) -> Option<[f64; 3]> {
    let mut found: Vec<[f64; 3]> =
        patches.iter().filter(|p| p.rgb.iter().zip(rgb).all(|(a, b)| (a - b).abs() < 0.01)).map(|p| p.xyz).collect();
    found.sort_by(|a, b| a[1].total_cmp(&b[1]));
    found.get(found.len().checked_sub(1)? / 2).copied()
}

impl Reference {
    /// From the patches measured. They must include white, black and 50%
    /// gray, as a report's and a profile's do.
    pub fn from_patches(about: About, patches: Vec<Patch>) -> Result<Reference, String> {
        let get = |rgb: [f64; 3], what: &str| median(&patches, rgb).ok_or(format!("The measurements have no {what}"));
        let white = get([100.0; 3], "white")?;
        let black = get([0.0; 3], "black")?;
        let gray = get([50.0; 3], "50% gray")?;
        if !(gray[1] > black[1] && white[1] > gray[1]) {
            return Err("White measured no brighter than black. Was the instrument on the screen?".into());
        }
        // The panel's black glow is in every reading; left in, it pulls the
        // primaries toward white
        let primary = |rgb| median(&patches, rgb).map(|c: [f64; 3]| xy([0, 1, 2].map(|i| c[i] - black[i])));
        let primaries = match [[100.0, 0.0, 0.0], [0.0, 100.0, 0.0], [0.0, 0.0, 100.0]].map(primary) {
            [Some(r), Some(g), Some(b)] => Some([r, g, b]),
            _ => None,
        };
        let [white_x, white_y] = xy(white);
        Ok(Reference {
            format: FORMAT,
            display: about.display,
            model: about.model,
            measured: about.measured,
            mode: about.mode,
            profile: about.profile,
            instrument: about.instrument,
            correction: about.correction,
            white: white[1],
            white_x,
            white_y,
            cct: report::cct((white_x, white_y)),
            black: black[1],
            black_target: about.black_target,
            gray50: gray[1],
            gamma50: gamma50(gray[1], white[1]),
            primaries,
            patches,
        })
    }

    fn text(&self) -> Result<String, String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        Ok(format!("# DisplayCraft display reference\n{text}"))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.text().map_err(std::io::Error::other)?)
    }

    /// Put the reference in the profile `icc`. The profile is replaced
    /// whole, by renaming, so a failure leaves it as it was.
    pub fn save_in_profile(&self, icc: &Path) -> Result<(), String> {
        let b = std::fs::read(icc).map_err(|e| e.to_string())?;
        let with = profiles::add_data_tag(&b, TAG, self.text()?.as_bytes()).ok_or("the profile can't take it")?;
        let tmp = icc.with_extension("icc-new");
        std::fs::write(&tmp, with).and_then(|()| std::fs::rename(&tmp, icc)).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }

    /// From a .dcref file, or from a profile DisplayCraft made
    pub fn load(path: &Path) -> Result<Reference, String> {
        let b = std::fs::read(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
        let text = if profiles::parse(&b).is_some() {
            let Some(data) = profiles::data_tag(&b, TAG) else {
                // Profiles made before references went in them have one beside
                let beside = path.with_extension("dcref");
                if beside.is_file() {
                    return Reference::load(&beside);
                }
                return Err("The profile has no DisplayCraft reference".into());
            };
            String::from_utf8(data).map_err(|_| "The profile's reference is damaged".to_string())?
        } else {
            String::from_utf8(b).map_err(|_| "Not a DisplayCraft reference or a profile".to_string())?
        };
        let r: Reference = toml::from_str(&text).map_err(|e| format!("Not a DisplayCraft reference: {}", e.message()))?;
        if r.format > FORMAT {
            return Err("The reference was saved by a newer DisplayCraft".into());
        }
        // Guards the logarithms and the targets made from them
        if !(r.white > 0.0 && r.gray50 > 0.0 && r.gray50 < r.white && r.white_y > 0.0) {
            return Err("The reference's measurements don't make sense".into());
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn about() -> About {
        About {
            display: "DP-3 (ASUSTek COMPUTER INC PA279CRV)".into(),
            model: Some("ASUSTek COMPUTER INC PA279CRV".into()),
            measured: "2026-10-14 15:30".into(),
            mode: "as calibrated now".into(),
            profile: None,
            instrument: "i1 DisplayPro, ColorMunki Display".into(),
            correction: Some("WLEDFamily_07Feb11.ccss".into()),
            black_target: Some(0.5),
        }
    }

    /// A report's patches on a display with gamma 2.2 from 0.2 to 160.2 cd/m²
    fn patches() -> Vec<Patch> {
        report::PATCHES
            .iter()
            .map(|&rgb| {
                let y = 0.2 + 160.0 * (rgb[0].max(rgb[1]).max(rgb[2]) / 100.0).powf(2.2);
                Patch { rgb, xyz: [y * 0.9505, y, y * 1.089] }
            })
            .collect()
    }

    #[test]
    fn gamma_from_the_50_percent_point() {
        assert!((gamma50(0.5f64.powf(2.2), 1.0) - 2.2).abs() < 1e-12);
        // 22% of white at 50% is about gamma 2.18
        assert!((gamma50(22.0, 100.0) - 2.184).abs() < 0.001);
    }

    #[test]
    fn model_from_the_name() {
        assert_eq!(model("eDP-1 (Sharp Corporation eDP-1)").as_deref(), Some("Sharp Corporation eDP-1"));
        assert_eq!(model("Built-in Display"), None);
    }

    #[test]
    fn black_target_from_a_cal() {
        let cal = "CAL\n\nTARGET_GAMMA \"2.200000\"\nTARGET_BLACK_BRIGHTNESS \"0.450000\"\nQUALITY \"medium\"\n";
        assert_eq!(cal_black_target(cal), 0.45);
        assert_eq!(cal_black_target("CAL\n\nTARGET_GAMMA \"2.200000\"\n"), 0.0);
    }

    #[test]
    fn from_a_profiles_measurements() {
        // As dispread writes it: white Y = 100, then the calibration
        let ti3 = "CTI3\n\nLUMINANCE_XYZ_CDM2 \"190.0 200.0 210.0\"\n\nNUMBER_OF_FIELDS 7\nBEGIN_DATA_FORMAT\n\
                   SAMPLE_ID RGB_R RGB_G RGB_B XYZ_X XYZ_Y XYZ_Z\nEND_DATA_FORMAT\n\nNUMBER_OF_SETS 5\nBEGIN_DATA\n\
                   1 100.0000 100.0000 100.0000 95.0 100.0 105.0\n2 100.0000 100.0000 100.0000 95.1 100.1 105.1\n\
                   3 0.00000 0.00000 0.00000 0.05 0.06 0.09\n4 50.00000 50.00000 50.00000 20.9 22.0 23.1\n\
                   5 100.0000 100.0000 100.0000 94.9 99.9 104.9\nEND_DATA\n\nCAL\n\nBEGIN_DATA\n0.0 0.0 0.0 0.0\nEND_DATA\n";
        let patches = read_ti3(ti3).unwrap();
        assert_eq!(patches.len(), 5);
        let r = Reference::from_patches(about(), patches).unwrap();
        // The median of three whites, made absolute
        assert!((r.white - 200.0).abs() < 1e-9, "{}", r.white);
        assert!((r.gray50 - 44.0).abs() < 1e-9 && (r.black - 0.12).abs() < 1e-9);
        assert_eq!(r.primaries, None);
        let missing = read_ti3(&ti3.replace("4 50.00000 50.00000 50.00000", "4 40.00000 40.00000 40.00000")).unwrap();
        assert!(Reference::from_patches(about(), missing).is_err());
    }

    #[test]
    fn round_trips() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/test-tmp-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test-reference.dcref");
        let r = Reference::from_patches(about(), patches()).unwrap();
        assert!((r.gray50 - (0.2 + 160.0 * 0.5f64.powf(2.2))).abs() < 1e-9);
        assert!((r.black - 0.2).abs() < 1e-9);
        assert!(r.primaries.is_some());
        r.save(&path).unwrap();
        assert_eq!(Reference::load(&path).unwrap(), r);
        std::fs::write(&path, "white = 3\n").unwrap();
        assert!(Reference::load(&path).is_err());
        // Without what isn't always known
        let unknown = Reference { black_target: None, model: None, primaries: None, ..r.clone() };
        unknown.save(&path).unwrap();
        assert_eq!(Reference::load(&path).unwrap(), unknown);
        // In a profile: a bare header and an empty tag table
        let icc = dir.join("test-profile.icc");
        let mut b = vec![0u8; 132];
        b[0..4].copy_from_slice(&132u32.to_be_bytes());
        b[36..40].copy_from_slice(b"acsp");
        std::fs::write(&icc, &b).unwrap();
        assert_eq!(Reference::load(&icc).unwrap_err(), "The profile has no DisplayCraft reference");
        r.save_in_profile(&icc).unwrap();
        assert_eq!(Reference::load(&icc).unwrap(), r);
        assert!(r.save_in_profile(&icc).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
