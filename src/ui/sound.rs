//! Sound in the window: what's playing, the transport, and waveforms to look at and click on.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::asset::AssetKind;
use crate::audio::{NowPlaying, Player, ReadAhead, Source, Waveform};
use crate::diag;
use crate::library::Library;

use super::format::format_time;
use super::{theme, widgets};

/// Sounds read into memory ahead of the selection, in the direction of travel.
const READ_AHEAD_SOUNDS: usize = 3;

pub struct Sound {
    pub player: Option<Player>,
    read_ahead: ReadAhead,
    /// The entry last sent to the player: (index, key).
    pub playing: Option<(u32, i64)>,
    pub autoplay: bool,
    pub looping: bool,
    pub volume: f32,
}

impl Sound {
    pub fn start(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        let player = Player::start(Arc::new(move || ctx.request_repaint()))
            .map_err(|err| diag::report("No sound output", err))
            .ok();
        let sound =
            Self { player, read_ahead: ReadAhead::start(), playing: None, autoplay: true, looping: false, volume: 0.8 };
        sound.set_volume(sound.volume);
        sound
    }

    pub fn now(&self) -> Option<NowPlaying> {
        self.player.as_ref().and_then(Player::now)
    }

    /// What's playing, if it's `key` and hasn't finished.
    pub fn now_for(&self, key: i64) -> Option<NowPlaying> {
        self.now().filter(|now| now.key == key && !now.finished)
    }

    pub fn is_audible(&self) -> bool {
        self.now().is_some_and(|now| !now.finished && !now.paused)
    }

    pub fn play(&mut self, library: &Library, index: u32) {
        let Some(player) = &self.player else { return };
        let entry = &library.entries[index as usize];
        let source = match self.read_ahead.get(entry.key) {
            Some(bytes) => Source::Memory(bytes),
            None => Source::File(library.path_of(entry)),
        };
        player.play(entry.key, source, entry.extension());
        self.playing = Some((index, entry.key));
    }

    /// Plays `index` from `seconds` in, or jumps there if it's already playing.
    pub fn play_from(&mut self, library: &Library, index: u32, seconds: f64) {
        let key = library.entries[index as usize].key;
        if self.now_for(key).is_none() {
            self.play(library, index);
        }
        if let Some(player) = &self.player {
            player.seek(seconds);
        }
    }

    /// Pauses or resumes `index`, or starts it if it isn't the one playing.
    pub fn toggle(&mut self, library: &Library, index: u32) {
        let key = library.entries[index as usize].key;
        match (&self.player, self.now_for(key)) {
            (Some(player), Some(now)) => player.set_paused(!now.paused),
            _ => self.play(library, index),
        }
    }

    pub fn stop(&self) {
        if let Some(player) = &self.player {
            player.stop();
        }
    }

    pub fn set_volume(&self, volume: f32) {
        if let Some(player) = &self.player {
            player.set_volume(volume);
        }
    }

    pub fn set_looping(&self, looping: bool) {
        if let Some(player) = &self.player {
            player.set_looping(looping);
        }
    }

    /// Reads the sounds after `index` in `view` (and one before) into memory, so moving to them
    /// starts without a disk wait.
    pub fn read_ahead_around(&self, library: &Library, view: &[u32], index: u32) {
        let Some(position) = view.iter().position(|&i| i == index) else { return };
        let is_sound = |&&i: &&u32| library.entries[i as usize].kind == AssetKind::Audio;
        let after = view[position + 1..].iter().filter(is_sound).take(READ_AHEAD_SOUNDS);
        let before = view[..position].iter().rev().filter(is_sound).take(1);
        let files = after
            .chain(before)
            .map(|&i| {
                let entry = &library.entries[i as usize];
                (entry.key, library.path_of(entry), entry.size)
            })
            .collect();
        self.read_ahead.want(files);
    }

    /// A rescan changed entry indices; keys stay.
    pub fn remap(&mut self, new_index: &HashMap<i64, u32>) {
        self.playing = self.playing.and_then(|(_, key)| new_index.get(&key).map(|&index| (index, key)));
    }

    /// A different folder was opened.
    pub fn reset(&mut self) {
        self.playing = None;
        self.stop();
    }
}

/// Play/pause, stop, loop, the time and the volume for the sound `index`.
pub fn transport(ui: &mut Ui, sound: &mut Sound, library: &Library, index: u32, duration: Option<f64>) {
    let entry = &library.entries[index as usize];
    if sound.player.is_none() {
        ui.colored_label(theme::ERROR, "No sound output. See the error in the status bar.");
        return;
    }
    let now = sound.now_for(entry.key);
    let audible = now.as_ref().is_some_and(|now| !now.paused);
    let volume_inline = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (glyph, tip) = if audible { (icon::PAUSE, "Pause") } else { (icon::PLAY, "Play") };
            let hint = |action| crate::keymap::hint(ui.ctx(), action);
            let (play_hint, stop_hint) =
                (hint(crate::keymap::Action::PlayPause), hint(crate::keymap::Action::StopOrClear));
            if play_button(ui, glyph).on_hover_text(format!("{tip}{play_hint}")).clicked() {
                sound.toggle(library, index);
            }
            if widgets::icon_button(ui, icon::STOP, &format!("Stop{stop_hint}"), false).clicked() {
                sound.stop();
            }
            if widgets::icon_button(ui, icon::REPEAT, "Loop", sound.looping).clicked() {
                sound.looping = !sound.looping;
                sound.set_looping(sound.looping);
            }
            ui.add_space(6.0);
            let position = now.as_ref().map_or(0.0, |now| now.position);
            ui.label(egui::RichText::new(clock(position, duration)).monospace().size(12.0).color(theme::TEXT_WEAK));
            // The volume goes on its own line when the pane is too narrow for it here.
            let inline = ui.available_width() >= 150.0;
            if inline {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| volume(ui, sound, 90.0));
            }
            inline
        })
        .inner;
    if !volume_inline {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let width = (ui.available_width() - 30.0).clamp(60.0, 160.0);
                volume(ui, sound, width);
            });
        });
    }
    if let Some(error) = now.as_ref().and_then(|now| now.error.as_deref()) {
        ui.colored_label(theme::ERROR, error);
    }
}

/// The volume slider with a speaker icon to its left, in a right-to-left layout.
fn volume(ui: &mut Ui, sound: &mut Sound, width: f32) {
    let slider = egui::Slider::new(&mut sound.volume, 0.0..=1.0).show_value(false);
    if ui.add_sized(vec2(width, 20.0), slider).changed() {
        sound.set_volume(sound.volume);
    }
    let glyph = if sound.volume <= 0.001 {
        icon::SPEAKER_NONE
    } else if sound.volume < 0.5 {
        icon::SPEAKER_LOW
    } else {
        icon::SPEAKER_HIGH
    };
    let name = sound.player.as_ref().map_or("", |player| player.device_name.as_str());
    ui.label(egui::RichText::new(glyph).size(16.0).color(theme::TEXT_WEAK)).on_hover_text(name);
}

/// "0:42 / 2:00" for long sounds, "0.4 / 1.2 s" for short ones.
fn clock(position: f64, duration: Option<f64>) -> String {
    let Some(total) = duration else { return format_time(position) };
    if total >= 60.0 {
        let minutes = |seconds: f64| {
            let whole = seconds.max(0.0) as u64;
            format!("{}:{:02}", whole / 60, whole % 60)
        };
        format!("{} / {}", minutes(position), minutes(total))
    } else {
        format!("{:.1} / {:.1} s", position.max(0.0), total)
    }
}

/// The round accent play button.
fn play_button(ui: &mut Ui, glyph: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let fill = if response.hovered() { theme::ACCENT } else { theme::ACCENT.gamma_multiply(0.85) };
    ui.painter().circle_filled(rect.center(), 15.0, fill);
    let nudge = if glyph == icon::PLAY { vec2(1.0, 0.0) } else { Vec2::ZERO };
    ui.painter().text(rect.center() + nudge, Align2::CENTER_CENTER, glyph, FontId::proportional(16.0), Color32::WHITE);
    response
}

/// A waveform to click or drag on to jump. Starts the sound if it isn't playing.
pub fn scrubber(
    ui: &mut Ui,
    height: f32,
    sound: &mut Sound,
    library: &Library,
    index: u32,
    waveform: Option<&Waveform>,
    failed: bool,
) {
    let entry = &library.entries[index as usize];
    let now = sound.now_for(entry.key);
    let duration = now.as_ref().and_then(|now| now.duration).or(waveform.map(|w| w.duration as f64));
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::CARD_RADIUS, theme::WELL);
    let progress = match (&now, duration) {
        (Some(now), Some(total)) if total > 0.0 => Some((now.position / total) as f32),
        _ => None,
    };
    let color = entry.kind.color();
    if let Some(waveform) = waveform {
        draw_waveform(&painter, rect.shrink2(vec2(8.0, 10.0)), waveform, color.gamma_multiply(0.45), color, progress);
    } else {
        let text = if failed { "Couldn't read this sound" } else { "Reading…" };
        painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(13.0), theme::TEXT_FAINT);
    }
    let inner = rect.shrink2(vec2(8.0, 0.0));
    let fraction_at = |x: f32| ((x - inner.left()) / inner.width()).clamp(0.0, 1.0);
    if let Some(progress) = progress {
        let x = inner.left() + progress * inner.width();
        painter
            .line_segment([pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)], Stroke::new(2.0, Color32::WHITE));
    }
    if let Some(pointer) = response.interact_pointer_pos().or(response.hover_pos()) {
        // Where a click would jump to.
        let x = inner.left() + fraction_at(pointer.x) * inner.width();
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, theme::TEXT.gamma_multiply(0.4)),
        );
        if let Some(total) = duration {
            painter.text(
                pos2(x + 4.0, rect.top() + 4.0),
                Align2::LEFT_TOP,
                format_time(fraction_at(pointer.x) as f64 * total),
                FontId::proportional(11.0),
                theme::TEXT_WEAK,
            );
        }
    }
    if (response.clicked() || response.drag_stopped())
        && let (Some(pointer), Some(total)) = (response.interact_pointer_pos(), duration)
    {
        sound.play_from(library, index, fraction_at(pointer.x) as f64 * total);
    }
}

/// Draws a waveform as one mesh: a bar per column from the lowest to the highest sample in it.
/// Columns before `progress` (0..1) use `played`.
pub fn draw_waveform(
    painter: &egui::Painter,
    rect: Rect,
    waveform: &Waveform,
    color: Color32,
    played: Color32,
    progress: Option<f32>,
) {
    let peaks = &waveform.peaks;
    if peaks.is_empty() || rect.width() < 1.0 {
        return;
    }
    let columns = rect.width() as usize;
    let column_width = rect.width() / columns as f32;
    let (middle, half) = (rect.center().y, rect.height() / 2.0);
    let played_until = progress.map(|p| rect.left() + p * rect.width());
    let mut mesh = egui::Mesh::default();
    for column in 0..columns {
        let start = column * peaks.len() / columns;
        let end = ((column + 1) * peaks.len() / columns).clamp(start + 1, peaks.len());
        let (low, high) =
            peaks[start..end].iter().fold((i8::MAX, i8::MIN), |(low, high), [l, h]| (low.min(*l), high.max(*h)));
        let x = rect.left() + column as f32 * column_width;
        // Silence still shows as a thin line.
        let top = (middle - high as f32 / 127.0 * half).min(middle - 0.5);
        let bottom = (middle - low as f32 / 127.0 * half).max(middle + 0.5);
        let fill = if played_until.is_some_and(|until| x < until) { played } else { color };
        mesh.add_colored_rect(Rect::from_min_max(pos2(x, top), pos2(x + column_width, bottom)), fill);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A small "now playing" control for the status bar, for when the playing sound isn't the one
/// shown. Returns the entry clicked, to show it.
pub fn now_playing_chip(ui: &mut Ui, sound: &mut Sound, library: &Library) -> Option<u32> {
    let (index, key) = sound.playing?;
    let now = sound.now_for(key)?;
    let entry = library.entries.get(index as usize).filter(|entry| entry.key == key)?;
    let glyph = if now.paused { icon::PLAY } else { icon::PAUSE };
    if widgets::icon_button_sized(ui, glyph, "Play or pause", false, 22.0).clicked() {
        sound.toggle(library, index);
    }
    let progress =
        now.duration.map_or(String::new(), |total| format!("  {} / {}", format_time(now.position), format_time(total)));
    let text = egui::RichText::new(format!("{}{progress}", entry.name())).size(12.0).color(theme::TEXT_WEAK);
    let clicked = ui.add(egui::Label::new(text).sense(Sense::click())).on_hover_text("Show this sound").clicked();
    clicked.then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_matches_the_length() {
        assert_eq!(clock(42.4, Some(120.0)), "0:42 / 2:00");
        assert_eq!(clock(0.4, Some(1.25)), "0.4 / 1.2 s");
        assert_eq!(clock(3.0, None), "3.0 s");
    }
}
