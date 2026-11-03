//! Assets dragged out of the window: once the pointer leaves it with them, the drag is handed to
//! Windows, which carries the files into Explorer or another app, as a copy (`platform::drag_files_out`).
//! Inside the window it stays the app's own drag, onto a collection or category.

use std::path::PathBuf;

use eframe::egui;

use super::BrowserApp;
use crate::ui::organize::DraggedAssets;
use crate::{diag, platform};

/// What a drag out of the window leaves to tidy up in the frame after it.
#[derive(Default)]
pub(super) struct DragOut {
    /// Where egui last saw the pointer: the OS's drag took the button's release, and egui gets it
    /// back there.
    release_at: Option<egui::Pos2>,
    /// The files that went, so dropping them back on the window isn't taken for files from outside.
    dragged: Vec<PathBuf>,
}

impl BrowserApp {
    /// Hands a drag of assets to Windows once the pointer leaves the window with them. Returns when
    /// they're dropped or the drag is given up.
    pub(super) fn drag_out(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(assets) = egui::DragAndDrop::payload::<DraggedAssets>(ctx) else { return };
        let (pointer, held) = ctx.input(|input| (input.pointer.latest_pos(), input.pointer.primary_down()));
        let window = ctx.content_rect();
        if !left_the_window(pointer, window, held) {
            return;
        }
        egui::DragAndDrop::clear_payload(ctx);
        self.chrome.drag_out.started(pointer.unwrap_or(window.center()), assets.0.clone());
        if let Err(err) = platform::drag_files_out(frame, &assets.0) {
            diag::report("Dragging files out", err);
        }
        ctx.request_repaint();
    }
}

impl DragOut {
    fn started(&mut self, pointer: egui::Pos2, paths: Vec<PathBuf>) {
        self.release_at = Some(pointer);
        self.dragged = paths;
    }

    /// Before the frame after a drag out: gives egui back the button's release, without which it
    /// would think the drag goes on, and drops files that were only dragged out and back.
    pub(super) fn tidy_up(&mut self, input: &mut egui::RawInput) {
        let Some(pos) = self.release_at.take() else { return };
        let release = egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        };
        input.events.splice(0..0, [release, egui::Event::PointerGone]);
        let dragged = std::mem::take(&mut self.dragged);
        let dropped: Vec<PathBuf> = input.dropped_files.iter().map(|file| file.path().to_path_buf()).collect();
        if same_files(&dragged, &dropped) {
            input.dropped_files.clear();
        }
    }
}

/// Whether a drag has gone past the window's edge with the button still held. Let go outside the
/// window, it's over: handed to the OS then, it would drop wherever the pointer happens to be.
fn left_the_window(pointer: Option<egui::Pos2>, window: egui::Rect, held: bool) -> bool {
    held && pointer.is_none_or(|pos| !window.contains(pos))
}

fn same_files(a: &[PathBuf], b: &[PathBuf]) -> bool {
    a.len() == b.len() && a.iter().all(|path| b.contains(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_goes_out_once_it_leaves_the_window_held() {
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        assert!(!left_the_window(Some(egui::pos2(400.0, 300.0)), window, true), "inside");
        assert!(left_the_window(Some(egui::pos2(-4.0, 300.0)), window, true), "past the left edge");
        assert!(left_the_window(Some(egui::pos2(400.0, 612.0)), window, true), "below the window");
        assert!(left_the_window(None, window, true), "the pointer gone from the window");
        assert!(!left_the_window(Some(egui::pos2(-4.0, 300.0)), window, false), "let go outside: over");
    }

    #[test]
    fn after_a_drag_out_egui_gets_the_release_back() {
        let mut drag = DragOut::default();
        drag.started(egui::pos2(-4.0, 300.0), vec![PathBuf::from("F:/Assets/hero.png")]);
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerMoved(egui::pos2(10.0, 10.0)));
        drag.tidy_up(&mut input);
        assert!(
            matches!(input.events[0], egui::Event::PointerButton { pressed: false, button: egui::PointerButton::Primary, pos, .. } if pos == egui::pos2(-4.0, 300.0)),
            "the release first: {:?}",
            input.events
        );
        assert!(matches!(input.events[1], egui::Event::PointerGone));
        assert!(matches!(input.events[2], egui::Event::PointerMoved(_)), "what came after the drag stays");

        let mut next = egui::RawInput::default();
        drag.tidy_up(&mut next);
        assert!(next.events.is_empty(), "only once");
    }

    /// A file dropped on the window, as egui-winit hands it over.
    #[derive(Debug)]
    struct Dropped(PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("not read here".to_owned())
        }
    }

    #[test]
    fn files_dragged_out_and_back_in_are_not_taken_for_a_drop_from_outside() {
        let dragged = vec![PathBuf::from("F:/Assets/hero.png"), PathBuf::from("F:/Assets/jump.wav")];
        let dropped_file =
            |path: &str| -> egui::DroppedFileHandle { std::sync::Arc::new(Dropped(PathBuf::from(path))) };

        let mut drag = DragOut::default();
        drag.started(egui::Pos2::ZERO, dragged.clone());
        let mut input = egui::RawInput {
            dropped_files: vec![dropped_file("F:/Assets/jump.wav"), dropped_file("F:/Assets/hero.png")],
            ..Default::default()
        };
        drag.tidy_up(&mut input);
        assert!(input.dropped_files.is_empty(), "the files that went came back: nothing to add");

        let mut drag = DragOut::default();
        drag.started(egui::Pos2::ZERO, dragged);
        let mut input = egui::RawInput { dropped_files: vec![dropped_file("D:/Sounds")], ..Default::default() };
        drag.tidy_up(&mut input);
        assert_eq!(input.dropped_files.len(), 1, "a folder from elsewhere still goes in");
    }
}
