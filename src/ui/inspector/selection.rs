//! The pane with several assets selected, and with none.

use eframe::egui::{self, FontId, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icon;

use crate::asset::{AssetKind, KindSet};
use crate::library::Library;
use crate::ui::browse::{Look, PreviewOf, draw_preview};
use crate::ui::format::{format_size, grouped};
use crate::ui::mode::Selection;
use crate::ui::previews::Previews;
use crate::ui::{theme, widgets};

/// Thumbnails shown for a multiple selection; the rest are counted.
const STRIP_MAX: usize = 48;

/// N selected, their size, and their thumbnails to pick from.
pub(super) fn selection_strip(
    ui: &mut Ui,
    library: &Library,
    selection: &Selection,
    previews: &mut Previews,
    look: Look,
) -> Option<u32> {
    let mut items: Vec<u32> = selection.items.iter().copied().collect();
    items.sort_unstable();
    let total: u64 = items.iter().map(|&index| library.entry(index).size).sum();
    widgets::section_label(ui, &format!("{} selected · {}", grouped(items.len()), format_size(total)));
    let mut counts = [0usize; 3];
    for &index in &items {
        counts[library.entry(index).kind.index()] += 1;
    }
    let summary: Vec<String> = AssetKind::ALL
        .into_iter()
        .filter(|kind| counts[kind.index()] > 0)
        .map(|kind| format!("{} {}", grouped(counts[kind.index()]), kind.label().to_lowercase()))
        .collect();
    ui.label(RichText::new(summary.join(" · ")).color(theme::TEXT_WEAK));

    let mut clicked = None;
    let side = 52.0;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for &index in items.iter().take(STRIP_MAX) {
            let entry = library.entry(index);
            let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
            let focused = selection.focus == Some(index);
            ui.painter().rect_filled(
                rect,
                theme::RADIUS,
                if response.hovered() { theme::CARD_HOVER } else { theme::CARD },
            );
            let preview = PreviewOf { entry, previews, playing: None, large: false };
            draw_preview(ui.painter(), rect.shrink(4.0), &preview, look);
            if focused {
                ui.painter().rect_stroke(
                    rect,
                    theme::RADIUS,
                    egui::Stroke::new(1.5, theme::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            if response.on_hover_text(entry.name()).clicked() {
                clicked = Some(index);
            }
        }
        if items.len() > STRIP_MAX {
            ui.label(RichText::new(format!("+{}", grouped(items.len() - STRIP_MAX))).color(theme::TEXT_FAINT));
        }
    });
    for &index in items.iter().take(STRIP_MAX) {
        previews.want(index);
    }
    clicked
}

/// Shown when nothing is selected.
pub(super) fn empty_state(ui: &mut Ui, totals: [u32; 3], kinds: KindSet) {
    ui.add_space(ui.available_height() * 0.25);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon::CURSOR_CLICK).size(theme::size::PANE_ICON).color(theme::TEXT_FAINT));
        ui.add_space(6.0);
        ui.label(
            RichText::new("Nothing selected")
                .font(FontId::new(theme::size::TITLE, theme::semibold()))
                .color(theme::TEXT),
        );
        ui.label(
            RichText::new("Click an asset to see it here.\nCtrl- or Shift-click to pick several.")
                .size(theme::size::CAPTION)
                .color(theme::TEXT_WEAK),
        );
        ui.add_space(18.0);
        for kind in AssetKind::ALL.into_iter().filter(|kind| kinds.contains(*kind)) {
            let count = totals[kind.index()];
            let noun = if count == 1 { kind.singular() } else { kind.label() }.to_lowercase();
            ui.label(
                RichText::new(format!("{}  {} {noun}", theme::kind_icon(kind), grouped(count as usize)))
                    .size(theme::size::LABEL)
                    .color(if count > 0 { theme::TEXT_WEAK } else { theme::TEXT_FAINT }),
            );
        }
    });
}
