mod app;
mod audio;
#[cfg(all(feature = "ffmpeg", feature = "whisper"))]
mod caption_job;
mod composite;
mod dialogs;
mod preview;
mod proxy_job;
mod theme;
mod ui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1580.0, 960.0])
            .with_min_inner_size([1180.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Meridian",
        options,
        Box::new(|cc| Ok(Box::new(app::MeridianApp::new(cc)))),
    )
}
