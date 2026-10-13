//! Simple charts, drawn in the window with egui or written as SVG for the
//! HTML report, from one description.

use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Line { width: f32, dashed: bool },
    /// A closed outline, e.g. a gamut triangle
    Outline { width: f32, dashed: bool },
    /// Points with labels drawn next to them, e.g. white points
    Points { radius: f32 },
    /// Vertical bars from zero, one per point
    Bars,
}

#[derive(Clone, Debug)]
pub struct Series {
    pub name: String,
    pub color: [u8; 3],
    pub points: Vec<(f64, f64)>,
    pub style: Style,
    /// Colors per bar, when `Bars` bars differ
    pub bar_colors: Option<Vec<[u8; 3]>>,
}

impl Series {
    pub fn new(name: &str, color: [u8; 3], points: Vec<(f64, f64)>, style: Style) -> Series {
        Series { name: name.to_string(), color, points, style, bar_colors: None }
    }
}

#[derive(Clone, Debug)]
pub struct Axis {
    pub label: String,
    pub min: f64,
    pub max: f64,
    pub ticks: Vec<f64>,
}

impl Axis {
    pub fn new(label: &str, min: f64, max: f64, step: f64) -> Axis {
        let n = ((max - min) / step).round() as usize;
        Axis { label: label.to_string(), min, max, ticks: (0..=n).map(|i| min + i as f64 * step).collect() }
    }
}

#[derive(Clone, Debug)]
pub struct Chart {
    pub title: String,
    pub x: Axis,
    pub y: Axis,
    pub series: Vec<Series>,
    /// Keep one unit the same length on both axes (chromaticity diagrams)
    pub equal_scale: bool,
}

pub const RED: [u8; 3] = [226, 80, 80];
pub const GREEN: [u8; 3] = [80, 190, 100];
pub const BLUE: [u8; 3] = [90, 130, 245];
pub const GRAY: [u8; 3] = [150, 150, 150];

/// The CIE 1931 2° spectral locus, 380-700 nm in 5 nm steps, as (nm, x, y).
/// Computed from the color matching functions in ArgyllCMS (xicc/xspect.c).
pub const LOCUS: [(u32, f64, f64); 65] = [
    (380, 0.17411, 0.00496),
    (385, 0.17401, 0.00498),
    (390, 0.17380, 0.00492),
    (395, 0.17356, 0.00492),
    (400, 0.17334, 0.00480),
    (405, 0.17302, 0.00478),
    (410, 0.17258, 0.00480),
    (415, 0.17209, 0.00483),
    (420, 0.17141, 0.00510),
    (425, 0.17030, 0.00579),
    (430, 0.16888, 0.00690),
    (435, 0.16690, 0.00856),
    (440, 0.16441, 0.01086),
    (445, 0.16110, 0.01379),
    (450, 0.15664, 0.01770),
    (455, 0.15099, 0.02274),
    (460, 0.14396, 0.02970),
    (465, 0.13550, 0.03988),
    (470, 0.12412, 0.05780),
    (475, 0.10959, 0.08684),
    (480, 0.09129, 0.13270),
    (485, 0.06871, 0.20072),
    (490, 0.04539, 0.29498),
    (495, 0.02346, 0.41270),
    (500, 0.00817, 0.53842),
    (505, 0.00386, 0.65482),
    (510, 0.01387, 0.75019),
    (515, 0.03885, 0.81202),
    (520, 0.07430, 0.83380),
    (525, 0.11416, 0.82621),
    (530, 0.15472, 0.80586),
    (535, 0.19288, 0.78163),
    (540, 0.22962, 0.75433),
    (545, 0.26578, 0.72432),
    (550, 0.30160, 0.69231),
    (555, 0.33736, 0.65885),
    (560, 0.37310, 0.62445),
    (565, 0.40874, 0.58961),
    (570, 0.44406, 0.55471),
    (575, 0.47877, 0.52020),
    (580, 0.51249, 0.48659),
    (585, 0.54479, 0.45443),
    (590, 0.57515, 0.42423),
    (595, 0.60293, 0.39650),
    (600, 0.62704, 0.37249),
    (605, 0.64823, 0.35139),
    (610, 0.66576, 0.33401),
    (615, 0.68008, 0.31975),
    (620, 0.69150, 0.30834),
    (625, 0.70061, 0.29930),
    (630, 0.70792, 0.29203),
    (635, 0.71403, 0.28593),
    (640, 0.71903, 0.28093),
    (645, 0.72303, 0.27695),
    (650, 0.72599, 0.27401),
    (655, 0.72827, 0.27173),
    (660, 0.72997, 0.27003),
    (665, 0.73109, 0.26891),
    (670, 0.73199, 0.26801),
    (675, 0.73272, 0.26728),
    (680, 0.73342, 0.26658),
    (685, 0.73405, 0.26595),
    (690, 0.73439, 0.26561),
    (695, 0.73459, 0.26541),
    (700, 0.73469, 0.26531),
];

const MARGIN_LEFT: f32 = 42.0;
const MARGIN_BOTTOM: f32 = 34.0;
const MARGIN_TOP: f32 = 24.0;
const MARGIN_RIGHT: f32 = 10.0;

impl Chart {
    /// The plot area within a chart of the given size, honouring
    /// `equal_scale`, as (left, top, width, height)
    fn plot_area(&self, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let (mut pw, mut ph) = (w - MARGIN_LEFT - MARGIN_RIGHT, h - MARGIN_TOP - MARGIN_BOTTOM);
        if self.equal_scale {
            let (sx, sy) = (pw as f64 / (self.x.max - self.x.min), ph as f64 / (self.y.max - self.y.min));
            let s = sx.min(sy);
            pw = (s * (self.x.max - self.x.min)) as f32;
            ph = (s * (self.y.max - self.y.min)) as f32;
        }
        (MARGIN_LEFT, MARGIN_TOP, pw, ph)
    }

    fn map(&self, area: (f32, f32, f32, f32), (x, y): (f64, f64)) -> (f32, f32) {
        let (l, t, w, h) = area;
        let fx = ((x - self.x.min) / (self.x.max - self.x.min)) as f32;
        let fy = ((y - self.y.min) / (self.y.max - self.y.min)) as f32;
        (l + fx * w, t + (1.0 - fy) * h)
    }

    /// As many decimals as the tick step needs (0.25 needs two)
    fn tick_label(v: f64, axis: &Axis) -> String {
        let step = axis.ticks.get(1).map(|t| t - axis.ticks[0]).unwrap_or(1.0);
        let decimals = (0..4).find(|&d| {
            let scaled = step * 10f64.powi(d);
            (scaled - scaled.round()).abs() < 1e-9
        });
        format!("{v:.*}", decimals.unwrap_or(3) as usize)
    }

    /// Named lines and outlines, for the legend
    fn legend(&self) -> impl Iterator<Item = &Series> {
        self.series.iter().filter(|s| matches!(s.style, Style::Line { .. } | Style::Outline { .. }) && !s.name.is_empty())
    }

    /// Draw in `ui`, `width` points wide
    pub fn show(&self, ui: &mut egui::Ui, width: f32, height: f32) {
        use egui::{Color32, Pos2, Stroke};
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        let text_color = visuals.text_color();
        let grid = visuals.weak_text_color().gamma_multiply(0.35);
        let area = self.plot_area(width, height);
        let at = |p: (f64, f64)| {
            let (x, y) = self.map(area, p);
            Pos2::new(rect.left() + x, rect.top() + y)
        };
        let color = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
        let small = egui::FontId::proportional(10.5);
        painter.text(Pos2::new(rect.left() + area.0, rect.top() + 4.0), egui::Align2::LEFT_TOP, &self.title, egui::FontId::proportional(12.5), text_color);
        for &v in &self.x.ticks {
            let (a, b) = (at((v, self.y.min)), at((v, self.y.max)));
            painter.line_segment([a, b], Stroke::new(1.0, grid));
            painter.text(a + egui::vec2(0.0, 3.0), egui::Align2::CENTER_TOP, Self::tick_label(v, &self.x), small.clone(), text_color);
        }
        for &v in &self.y.ticks {
            let (a, b) = (at((self.x.min, v)), at((self.x.max, v)));
            painter.line_segment([a, b], Stroke::new(1.0, grid));
            painter.text(a - egui::vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, Self::tick_label(v, &self.y), small.clone(), text_color);
        }
        let bottom = at((self.x.min, self.y.min)).y + 16.0;
        painter.text(Pos2::new(at(((self.x.min + self.x.max) / 2.0, self.y.min)).x, bottom), egui::Align2::CENTER_TOP, &self.x.label, small.clone(), text_color);
        for s in &self.series {
            let pts: Vec<Pos2> = s.points.iter().map(|&p| at(p)).collect();
            let c = color(s.color);
            match s.style {
                Style::Line { width, dashed } | Style::Outline { width, dashed } => {
                    let mut pts = pts;
                    if matches!(s.style, Style::Outline { .. }) {
                        if let Some(&first) = pts.first() {
                            pts.push(first);
                        }
                    }
                    if dashed {
                        painter.extend(egui::Shape::dashed_line(&pts, Stroke::new(width, c), 5.0, 4.0));
                    } else {
                        painter.add(egui::Shape::line(pts, Stroke::new(width, c)));
                    }
                }
                Style::Points { radius } => {
                    for p in pts {
                        painter.circle(p, radius, c, Stroke::new(1.0, text_color));
                        painter.text(p + egui::vec2(radius + 3.0, 0.0), egui::Align2::LEFT_CENTER, &s.name, small.clone(), text_color);
                    }
                }
                Style::Bars => {
                    let n = s.points.len().max(1) as f32;
                    let bw = (area.2 / n * 0.7).max(1.0);
                    for (i, (&(x, y), p)) in s.points.iter().zip(&pts).enumerate() {
                        let base = at((x, self.y.min.max(0.0)));
                        let top = Pos2::new(p.x, p.y.max(rect.top() + area.1));
                        let bc = s.bar_colors.as_ref().and_then(|b| b.get(i)).map(|&b| color(b)).unwrap_or(c);
                        let _ = y;
                        painter.rect_filled(egui::Rect::from_min_max(Pos2::new(top.x - bw / 2.0, top.y), Pos2::new(top.x + bw / 2.0, base.y)), 1.0, bc);
                    }
                }
            }
        }
        // Legend in the plot's lower right corner, which every chart here
        // leaves empty
        let entries: Vec<&Series> = self.legend().collect();
        let row = 13.0;
        let widest = entries.iter().map(|s| painter.layout_no_wrap(s.name.clone(), small.clone(), text_color).size().x).fold(0.0, f32::max);
        let (right, bottom) = (rect.left() + area.0 + area.2 - 6.0, rect.top() + area.1 + area.3 - 6.0);
        for (i, s) in entries.iter().enumerate() {
            let y = bottom - (entries.len() - i) as f32 * row + row / 2.0;
            let x = right - widest;
            painter.text(Pos2::new(x, y), egui::Align2::LEFT_CENTER, &s.name, small.clone(), text_color);
            painter.line_segment([Pos2::new(x - 16.0, y), Pos2::new(x - 4.0, y)], Stroke::new(2.0, color(s.color)));
        }
    }

    /// SVG of the chart, for embedding in HTML, on a light background
    pub fn svg(&self, width: f32, height: f32) -> String {
        let area = self.plot_area(width, height);
        let mut s = String::new();
        let _ = write!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="sans-serif" font-size="10.5">"#);
        let _ = write!(s, r#"<text x="{}" y="15" font-size="12.5" font-weight="600">{}</text>"#, area.0, escape(&self.title));
        for &v in &self.x.ticks {
            let (a, b) = (self.map(area, (v, self.y.min)), self.map(area, (v, self.y.max)));
            let _ = write!(s, r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#ddd"/>"##, a.0, a.1, b.0, b.1);
            let _ = write!(s, r#"<text x="{:.1}" y="{:.1}" text-anchor="middle">{}</text>"#, a.0, a.1 + 13.0, Self::tick_label(v, &self.x));
        }
        for &v in &self.y.ticks {
            let (a, b) = (self.map(area, (self.x.min, v)), self.map(area, (self.x.max, v)));
            let _ = write!(s, r##"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="#ddd"/>"##, a.0, a.1, b.0, b.1);
            let _ = write!(s, r#"<text x="{:.1}" y="{:.1}" text-anchor="end">{}</text>"#, a.0 - 4.0, a.1 + 3.5, Self::tick_label(v, &self.y));
        }
        let mid = self.map(area, ((self.x.min + self.x.max) / 2.0, self.y.min));
        let _ = write!(s, r#"<text x="{:.1}" y="{:.1}" text-anchor="middle">{}</text>"#, mid.0, mid.1 + 27.0, escape(&self.x.label));
        for ser in &self.series {
            let rgb = |c: [u8; 3]| format!("rgb({},{},{})", c[0], c[1], c[2]);
            let pts: Vec<(f32, f32)> = ser.points.iter().map(|&p| self.map(area, p)).collect();
            let path = pts.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect::<Vec<_>>().join(" ");
            match ser.style {
                Style::Line { width, dashed } | Style::Outline { width, dashed } => {
                    let tag = if matches!(ser.style, Style::Outline { .. }) { "polygon" } else { "polyline" };
                    let dash = if dashed { r#" stroke-dasharray="5 4""# } else { "" };
                    let _ = write!(s, r#"<{tag} points="{path}" fill="none" stroke="{}" stroke-width="{width}"{dash}/>"#, rgb(ser.color));
                }
                Style::Points { radius } => {
                    for (x, y) in &pts {
                        let _ = write!(s, r#"<circle cx="{x:.1}" cy="{y:.1}" r="{radius}" fill="{}" stroke="black"/>"#, rgb(ser.color));
                        let _ = write!(s, r#"<text x="{:.1}" y="{:.1}">{}</text>"#, x + radius + 3.0, y + 3.5, escape(&ser.name));
                    }
                }
                Style::Bars => {
                    let bw = (area.2 / ser.points.len().max(1) as f32 * 0.7).max(1.0);
                    for (i, ((x, y), &(px, _))) in pts.iter().zip(&ser.points).enumerate() {
                        let base = self.map(area, (px, self.y.min.max(0.0))).1;
                        let top = y.max(area.1);
                        let c = ser.bar_colors.as_ref().and_then(|b| b.get(i)).copied().unwrap_or(ser.color);
                        let _ = write!(s, r#"<rect x="{:.1}" y="{top:.1}" width="{bw:.1}" height="{:.1}" fill="{}"/>"#, x - bw / 2.0, (base - top).max(0.0), rgb(c));
                    }
                }
            }
        }
        let entries: Vec<&Series> = self.legend().collect();
        let widest = entries.iter().map(|e| e.name.chars().count() as f32 * 5.8).fold(0.0, f32::max);
        let (right, bottom) = (area.0 + area.2 - 6.0, area.1 + area.3 - 6.0);
        for (i, e) in entries.iter().enumerate() {
            let y = bottom - (entries.len() - i) as f32 * 13.0 + 6.5;
            let x = right - widest;
            let _ = write!(s, r#"<text x="{x:.1}" y="{:.1}">{}</text>"#, y + 3.5, escape(&e.name));
            let _ = write!(s, r#"<line x1="{:.1}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" stroke="rgb({},{},{})" stroke-width="2"/>"#, x - 16.0, x - 4.0, e.color[0], e.color[1], e.color[2]);
        }
        s.push_str("</svg>");
        s
    }
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Calibration curves (output against input, 0-1)
pub fn calibration(c: &[Vec<f64>; 3]) -> Chart {
    curves_chart("Calibration curves", c, None)
}

/// Tonal response curves, with gamma 2.2 for comparison
pub fn tone(c: &[Vec<f64>; 3]) -> Chart {
    curves_chart("Tonal response", c, Some(2.2))
}

fn curves_chart(title: &str, c: &[Vec<f64>; 3], gamma: Option<f64>) -> Chart {
    let pts = |v: &Vec<f64>| v.iter().enumerate().map(|(i, &y)| (i as f64 / (v.len() - 1) as f64, y)).collect();
    let mut series = vec![];
    match gamma {
        Some(g) => series.push(Series::new(&format!("gamma {g}"), GRAY, (0..=50).map(|i| { let x = i as f64 / 50.0; (x, x.powf(g)) }).collect(), Style::Line { width: 1.0, dashed: true })),
        None => series.push(Series::new("", GRAY, vec![(0.0, 0.0), (1.0, 1.0)], Style::Line { width: 1.0, dashed: true })),
    }
    series.push(Series::new("R", RED, pts(&c[0]), Style::Line { width: 1.6, dashed: false }));
    series.push(Series::new("G", GREEN, pts(&c[1]), Style::Line { width: 1.6, dashed: false }));
    series.push(Series::new("B", BLUE, pts(&c[2]), Style::Line { width: 1.6, dashed: false }));
    Chart {
        title: title.to_string(),
        x: Axis::new("input", 0.0, 1.0, 0.25),
        y: Axis::new("output", 0.0, 1.0, 0.25),
        series,
        equal_scale: true,
    }
}

/// CIE 1931 xy diagram: the spectral locus, the reference gamuts, the
/// display's gamut and its white point
pub fn gamut(primaries: [(f64, f64); 3], white: Option<(f64, f64)>) -> Chart {
    let mut series = vec![Series::new("", GRAY, LOCUS.iter().map(|&(_, x, y)| (x, y)).collect(), Style::Outline { width: 1.0, dashed: false })];
    let refs: [(&str, [u8; 3]); 3] = [("sRGB", [120, 120, 120]), ("Display P3", [200, 150, 60]), ("Adobe RGB", [150, 100, 200])];
    for ((name, prims), (_, color)) in crate::report::REFERENCES.iter().zip(refs) {
        series.push(Series::new(name, color, prims.to_vec(), Style::Outline { width: 1.0, dashed: true }));
    }
    series.push(Series::new("This display", [60, 170, 230], primaries.to_vec(), Style::Outline { width: 2.0, dashed: false }));
    if let Some(w) = white {
        series.push(Series::new("white", [255, 255, 255], vec![w], Style::Points { radius: 3.0 }));
    }
    Chart {
        title: "Gamut (CIE 1931 xy)".into(),
        x: Axis::new("x", 0.0, 0.8, 0.1),
        y: Axis::new("y", 0.0, 0.9, 0.1),
        series,
        equal_scale: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locus_and_svg() {
        assert_eq!(LOCUS.first().map(|l| l.0), Some(380));
        assert_eq!(LOCUS.last().map(|l| l.0), Some(700));
        let svg = gamut([(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)], Some((0.3127, 0.3290))).svg(360.0, 380.0);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("This display") && svg.contains("<polygon"));
    }
}
