//! How KDE arranges the outputs, from `kscreen-doctor -j`, for drawing them
//! as System Settings does.

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
    /// Mode size in pixels, as the panel runs
    pub pixels: (u32, u32),
    pub primary: bool,
    pub enabled: bool,
    /// Shows another output's picture rather than its own
    pub mirrored: bool,
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
                // A quarter turn swaps the sides
                let (lw, lh) = if matches!(o.rotation, 2 | 8) { (h / scale, w / scale) } else { (w / scale, h / scale) };
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
                }
            })
            .collect(),
    )
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
        assert_eq!((o[1].width, o[1].height, o[1].pixels), (1200.0, 1920.0, (1920, 1200)), "rotated a quarter turn");
        assert!(!o[2].enabled && o[2].mirrored);
        assert!(parse("not json").is_none());
    }
}
