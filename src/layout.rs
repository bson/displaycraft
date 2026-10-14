//! How the displays are arranged, for drawing them as System Settings does:
//! from KDE (`kscreen-doctor -j`), or elsewhere from ArgyllCMS's display
//! list, which gives each one's position and size.

use std::process::{Command, Stdio};

use serde::Deserialize;

/// One output, in KDE's logical coordinates (what the desktop uses, after
/// scaling and rotation)
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    /// e.g. "eDP-1", the same name Argyll's display list gives
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Mode size in pixels, as the output is turned (1200 x 1920 for a
    /// 1920 x 1200 panel on its side)
    pub pixels: (u32, u32),
    pub primary: bool,
    pub enabled: bool,
    /// Shows another output's picture rather than its own
    pub mirrored: bool,
    /// Its place in ArgyllCMS's display list, when made from that list
    /// rather than matched by name
    pub argyll_index: Option<usize>,
}

#[derive(Deserialize)]
struct Doc {
    outputs: Vec<Raw>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    connected: bool,
    pos: Option<Xy>,
    size: Option<Wh>,
    #[serde(default = "one")]
    scale: f64,
    /// 1 none, 2 a quarter turn left, 4 half, 8 a quarter turn right
    #[serde(default = "one_u")]
    rotation: u32,
    #[serde(default)]
    priority: u32,
    #[serde(default)]
    replication_source: u32,
}

#[derive(Deserialize)]
struct Xy {
    x: f64,
    y: f64,
}

#[derive(Deserialize)]
struct Wh {
    width: f64,
    height: f64,
}

fn one() -> f64 {
    1.0
}

fn one_u() -> u32 {
    1
}

/// Connected outputs from `kscreen-doctor -j` output
pub fn parse(json: &str) -> Option<Vec<Output>> {
    let doc: Doc = serde_json::from_str(json).ok()?;
    Some(
        doc.outputs
            .into_iter()
            .filter(|o| o.connected)
            .map(|o| {
                let (w, h) = o.size.map(|s| (s.width, s.height)).unwrap_or((0.0, 0.0));
                let scale = if o.scale > 0.0 { o.scale } else { 1.0 };
                // A quarter turn swaps the sides, of the mode as shown too
                let (w, h) = if matches!(o.rotation, 2 | 8) { (h, w) } else { (w, h) };
                let (lw, lh) = (w / scale, h / scale);
                let (x, y) = o.pos.map(|p| (p.x, p.y)).unwrap_or((0.0, 0.0));
                Output {
                    name: o.name,
                    x,
                    y,
                    width: lw,
                    height: lh,
                    pixels: (w as u32, h as u32),
                    primary: o.priority == 1,
                    enabled: o.enabled,
                    mirrored: o.replication_source != 0,
                    argyll_index: None,
                }
            })
            .collect(),
    )
}

/// The name to show for an ArgyllCMS display description:
/// "Monitor 1, Output eDP-1 (Sharp Corporation eDP-1) at 0, 0, ..." ->
/// "eDP-1 (Sharp Corporation eDP-1)". On macOS there's no "Output", and a
/// comma before "at": "Built-in Display, at 0, 0, ..." -> "Built-in Display".
pub fn display_name(description: &str) -> String {
    let text = description.split(", ").find_map(|p| p.strip_prefix("Output ")).unwrap_or(description);
    text.split(" at ").next().unwrap_or(text).trim_end_matches([',', ' ']).to_string()
}

/// The arrangement from ArgyllCMS's display descriptions, where KDE can't
/// say: "... at 0, 0, width 2624, height 1696 (Primary Display)". Sizes are
/// the desktop's coordinates, so they're also what's shown as the size.
/// Displays whose description doesn't parse are left out.
pub fn from_argyll(descriptions: &[String]) -> Vec<Output> {
    descriptions
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let (_, place) = d.split_once(" at ")?;
            let nums: Vec<f64> = place
                .split([',', ' ', '('])
                .filter(|w| !w.is_empty() && !matches!(*w, "width" | "height"))
                .take(4)
                .map(|w| w.parse().ok())
                .collect::<Option<_>>()?;
            let [x, y, width, height] = nums[..] else { return None };
            Some(Output {
                name: display_name(d),
                x,
                y,
                width,
                height,
                pixels: (width as u32, height as u32),
                primary: d.contains("Primary"),
                enabled: true,
                mirrored: false,
                argyll_index: Some(i),
            })
        })
        .collect()
}

/// Ask KDE. None if kscreen-doctor isn't there or says nothing usable.
pub fn read() -> Option<Vec<Output>> {
    let out = Command::new("kscreen-doctor").arg("-j").stdin(Stdio::null()).output().ok()?;
    parse(&String::from_utf8_lossy(&out.stdout)).filter(|o| !o.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_arrangement() {
        let json = r#"{"outputs": [
            {"name": "eDP-1", "enabled": true, "connected": true, "pos": {"x": 0, "y": 360}, "size": {"width": 2560, "height": 1600},
             "scale": 1.25, "rotation": 1, "priority": 1, "replicationSource": 0, "modes": []},
            {"name": "DP-3", "enabled": true, "connected": true, "pos": {"x": 2048, "y": 0}, "size": {"width": 1920, "height": 1200},
             "scale": 1, "rotation": 2, "priority": 2, "replicationSource": 0},
            {"name": "HDMI-A-1", "enabled": false, "connected": true, "pos": {"x": 0, "y": 0}, "size": {"width": 1920, "height": 1080},
             "scale": 1, "rotation": 1, "priority": 0, "replicationSource": 1},
            {"name": "DP-4", "enabled": false, "connected": false}
        ], "screen": {}}"#;
        let o = parse(json).unwrap();
        assert_eq!(o.len(), 3, "disconnected outputs are left out");
        assert_eq!((o[0].width, o[0].height, o[0].primary), (2048.0, 1280.0, true));
        assert_eq!((o[1].width, o[1].height, o[1].pixels), (1200.0, 1920.0, (1200, 1920)), "rotated a quarter turn");
        assert!(!o[2].enabled && o[2].mirrored);
        assert!(parse("not json").is_none());
    }

    #[test]
    fn arrangement_from_argyll() {
        let o = from_argyll(&[
            "Built-in Display, at 0, 0, width 2624, height 1696 (Primary Display)".into(),
            "DELL U2413, at 2624, -200, width 1920, height 1200".into(),
            "Something odd".into(),
        ]);
        assert_eq!(o.len(), 2);
        assert_eq!((o[0].name.as_str(), o[0].width, o[0].height, o[0].primary), ("Built-in Display", 2624.0, 1696.0, true));
        assert_eq!((o[1].x, o[1].y, o[1].argyll_index, o[1].primary), (2624.0, -200.0, Some(1), false));
    }
}
