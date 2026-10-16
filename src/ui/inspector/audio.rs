//! A sound in the pane: its player, and its details.

use eframe::egui::{RichText, Ui};

use crate::library::{Entry, Library};
use crate::playback::Playback;
use crate::ui::format::{channels, format_time, sample_rate};
use crate::ui::previews::Previews;
use crate::ui::sound::{self, SoundArgs, SoundCommand};
use crate::ui::{theme, widgets};

use super::pending;

pub(super) fn sound_section(
    ui: &mut Ui,
    library: &Library,
    focus: u32,
    previews: &Previews,
    playback: &Playback,
) -> Option<SoundCommand> {
    let key = library.entry(focus).key;
    let sound = SoundArgs {
        playback,
        library,
        index: focus,
        waveform: previews.waveforms.get(&key).map(|waveform| waveform.as_ref()),
        failed: previews.failed.contains_key(&key),
    };
    sound::player(ui, 120.0, &sound)
}

pub(super) fn sound_properties(ui: &mut Ui, entry: &Entry, previews: &Previews) {
    if let Some(waveform) = previews.waveforms.get(&entry.key) {
        widgets::property(ui, "Length", format_time(waveform.duration as f64));
        widgets::property(ui, "Sample rate", sample_rate(waveform.sample_rate));
        widgets::property(ui, "Channels", channels(waveform.channels));
    } else if let Some(message) = previews.failed.get(&entry.key) {
        widgets::property(ui, "Preview", RichText::new(message).color(theme::ERROR));
    } else {
        for name in ["Length", "Sample rate", "Channels"] {
            widgets::property(ui, name, pending());
        }
    }
    widgets::property(ui, "Format", entry.extension().to_uppercase());
}
