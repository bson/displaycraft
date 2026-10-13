mod app;
mod argyll;
mod settings;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("displaycraft")
            .with_app_id("displaycraft")
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("displaycraft", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
