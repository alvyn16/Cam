//! Headless UI smoke test: drive the home view and every editor panel through
//! egui frames (no window), including selections, playback and the export
//! sheet, to catch panics and layout loops without needing a display.

use std::time::Duration;

use glide::app::editor::{Editor, Selection, Tab};
use glide::app::home::{self, Devices, Settings, Thumbs};
use glide::render::Gpu;

fn frame(ctx: &egui::Context, t: f64, f: impl FnMut(&mut egui::Ui)) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1320.0, 840.0))),
        time: Some(t),
        ..Default::default()
    };
    let _ = ctx.run_ui(input, f);
}

#[test]
fn editor_and_home_render_every_panel() {
    let dir = std::env::temp_dir().join(format!("glide-ui-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Smoke.glide");
    glide::synth::write_package(&path, 3.0, 1280, 800).unwrap();

    let gpu = Gpu::headless().unwrap();
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm, egui_wgpu::RendererOptions::default());
    let ctx = egui::Context::default();
    glide::app::theme::apply(&ctx);

    let mut ed = Editor::open(&path, gpu).unwrap();
    let mut t = 0.0;
    let mut back = false;
    for (tab, _) in Tab::ALL {
        ed.tab = tab;
        for _ in 0..2 {
            t += 0.016;
            frame(&ctx, t, |ui| ed.ui(ui, &mut renderer, &mut back));
        }
    }
    // Selections show their own inspector panels.
    let zoom = ed.project.timeline.zooms.first().map(|z| z.id);
    let slice = ed.project.timeline.slices[0].id;
    for sel in [Selection::Slice(slice), zoom.map_or(Selection::None, Selection::Zoom)] {
        ed.selection = sel;
        t += 0.016;
        frame(&ctx, t, |ui| ed.ui(ui, &mut renderer, &mut back));
    }
    // Edits: split, add a zoom, undo/redo.
    ed.seek(1500.0);
    ed.split();
    assert_eq!(ed.project.timeline.slices.len(), 2);
    let zooms = ed.project.timeline.zooms.len();
    ed.seek(400.0);
    ed.add_zoom();
    assert!(ed.project.timeline.zooms.len() >= zooms);
    ed.undo();
    ed.undo();
    assert_eq!(ed.project.timeline.slices.len(), 1);
    ed.redo();
    assert_eq!(ed.project.timeline.slices.len(), 2);

    // Playback advances the playhead.
    ed.seek(0.0);
    ed.toggle_play();
    std::thread::sleep(Duration::from_millis(120));
    assert!(ed.tick());
    assert!(ed.playhead > 50.0, "playhead {}", ed.playhead);
    ed.toggle_play();

    // The export sheet renders.
    ed.export.open = true;
    t += 0.016;
    frame(&ctx, t, |ui| ed.ui(ui, &mut renderer, &mut back));
    ed.save();
    assert!(!back);

    // Home with this folder as the library.
    let mut settings = Settings::default();
    settings.projects_dir = Some(dir.clone());
    let mut projects = home::scan_projects(&dir);
    assert_eq!(projects.len(), 1);
    let mut devices = Devices::default();
    let mut thumbs = Thumbs::default();
    for _ in 0..3 {
        t += 0.016;
        frame(&ctx, t, |ui| {
            home::ui(ui, &mut settings, &mut devices, &mut projects, &mut thumbs, Some("permission note"));
        });
    }
    let _ = std::fs::remove_dir_all(&dir);
}
