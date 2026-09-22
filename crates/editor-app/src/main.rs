mod app;
mod audio;
#[cfg(all(feature = "ffmpeg", feature = "whisper"))]
mod caption_job;
mod composite;
mod dialogs;
mod gpu_display;
mod preview;
mod proxy_job;
mod theme;
mod ui;

fn native_options() -> eframe::NativeOptions {
    #[cfg_attr(not(feature = "wgpu"), allow(unused_mut))]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1580.0, 960.0])
            .with_min_inner_size([1180.0, 720.0]),
        ..Default::default()
    };
    // Default eframe stays on glow, and the program monitor uploads with
    // texSubImage2D. `--features wgpu` switches the window renderer so the
    // same composite goes through queue.write_texture instead.
    #[cfg(feature = "wgpu")]
    {
        options.renderer = eframe::Renderer::Wgpu;
    }
    options
}

fn main() -> eframe::Result {
    eframe::run_native(
        "Meridian",
        native_options(),
        Box::new(|cc| Ok(Box::new(app::MeridianApp::new(cc)))),
    )
}
