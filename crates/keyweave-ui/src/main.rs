fn main() -> eframe::Result {
    let smoke = std::env::args_os().skip(1).collect::<Vec<_>>();
    let smoke = match smoke.as_slice() {
        [] => false,
        [arg] if arg == "--smoke" => true,
        _ => {
            eprintln!("usage: keyweave-ui [--smoke]");
            std::process::exit(64);
        }
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 760.0])
            .with_min_inner_size([760.0, 600.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "KeyWeave Settings",
        options,
        Box::new(move |_| {
            let mut app = keyweave_ui::view::SettingsApp::default();
            if smoke {
                app.smoke_passes = Some(3);
            }
            Ok(Box::new(app))
        }),
    )
}
