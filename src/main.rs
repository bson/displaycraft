mod adjust;
mod app;
mod argyll;
mod charts;
mod corrections;
mod session;
mod profiles;
mod report;
mod settings;
mod targets;
mod theme;
mod verify;

fn icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/hicolor/256x256/apps/displaycraft.png"))
        .expect("the embedded icon is a valid PNG")
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DisplayCraft")
            .with_app_id("displaycraft")
            // Wayland takes the icon from the installed desktop entry
            // matching the app id; this covers running uninstalled
            .with_icon(icon())
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    // Whether ArgyllCMS can run here is checked once the window is up (see
    // App::refresh_inventory). The window itself needs Wayland, so without
    // it there's nowhere to say so but the terminal and dialog tools.
    let result = eframe::run_native("displaycraft", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))));
    if let Err(e) = &result {
        session::report_without_window(&format!(
            "DisplayCraft couldn't open its window ({e}). It needs KDE Plasma on Wayland."
        ));
    }
    result
}
