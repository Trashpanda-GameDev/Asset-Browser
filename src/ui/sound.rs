//! Sound in the window: the transport, waveforms to look at and click on, and what's playing.

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::audio::{NowPlaying, Waveform};
use crate::library::Library;
use crate::playback::Playback;

use super::format::format_time;
use super::keymap::{self, Action};
use super::{theme, widgets};

/// What's asked of the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoundCommand {
    /// Pause or resume the sound, or start it if it isn't the one playing.
    Toggle(u32),
    Stop,
    /// Play the sound from this many seconds in, or jump there if it's the one playing.
    PlayFrom(u32, f64),
    Loop(bool),
    Volume(f32),
}

/// A sound for the player, and what's known of it.
#[derive(Clone, Copy)]
pub struct SoundArgs<'a> {
    pub playback: &'a Playback,
    pub library: &'a Library,
    pub index: u32,
    /// Its waveform, once it's read.
    pub waveform: Option<&'a Waveform>,
    /// It couldn't be read.
    pub failed: bool,
}

impl SoundArgs<'_> {
    /// What's playing, if it's this sound.
    fn now(&self) -> Option<NowPlaying> {
        self.playback.now_for(self.library.entry(self.index).key)
    }

    /// How long the sound is: as the player has it, or else as its waveform says.
    fn length(&self, now: Option<&NowPlaying>) -> Option<f64> {
        now.and_then(|now| now.duration).or(self.waveform.map(|waveform| waveform.duration as f64))
    }
}

/// The waveform to click on, `height` tall, with the transport under it: the preview pane's
/// player, and the bar along the bottom while the pane is hidden. What was asked for.
pub fn player(ui: &mut Ui, height: f32, sound: &SoundArgs<'_>) -> Option<SoundCommand> {
    let scrubbed = scrubber(ui, height, sound);
    scrubbed.or(transport(ui, sound))
}

/// Play/pause, stop, loop, the time and the volume; what was asked for.
fn transport(ui: &mut Ui, sound: &SoundArgs<'_>) -> Option<SoundCommand> {
    let (playback, index) = (sound.playback, sound.index);
    if playback.player.is_none() {
        ui.colored_label(theme::ERROR, "No sound output. See the error in the status bar.");
        return None;
    }
    let now = sound.now();
    let duration = sound.length(now.as_ref());
    let audible = now.as_ref().is_some_and(|now| !now.paused);
    let mut command = None;
    let volume_inline = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (glyph, tip) = if audible { (icon::PAUSE, "Pause") } else { (icon::PLAY, "Play") };
            let play_tip = keymap::tooltip(ui.ctx(), tip, Action::PlayPause);
            if play_button(ui, glyph).on_hover_text(play_tip).clicked() {
                command = Some(SoundCommand::Toggle(index));
            }
            let stop_tip = keymap::tooltip(ui.ctx(), "Stop", Action::StopOrClear);
            if widgets::icon_button(ui, icon::STOP, &stop_tip, false).clicked() {
                command = Some(SoundCommand::Stop);
            }
            if widgets::icon_button(ui, icon::REPEAT, "Loop", playback.looping()).clicked() {
                command = Some(SoundCommand::Loop(!playback.looping()));
            }
            ui.add_space(6.0);
            let position = now.as_ref().map_or(0.0, |now| now.position);
            ui.label(
                egui::RichText::new(clock(position, duration))
                    .monospace()
                    .size(theme::size::CAPTION)
                    .color(theme::TEXT_WEAK),
            );
            // The volume goes on its own line when the pane is too narrow for it here.
            let inline = ui.available_width() >= 150.0;
            if inline {
                let right_to_left = egui::Layout::right_to_left(egui::Align::Center);
                command = command.or(ui.with_layout(right_to_left, |ui| volume(ui, playback, 90.0)).inner);
            }
            inline
        })
        .inner;
    if !volume_inline {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let width = (ui.available_width() - 30.0).clamp(60.0, 160.0);
                command = command.or(volume(ui, playback, width));
            });
        });
    }
    if let Some(error) = now.as_ref().and_then(|now| now.error.as_deref()) {
        ui.colored_label(theme::ERROR, error);
    }
    command
}

/// The volume slider with a speaker icon to its left, in a right-to-left layout; the volume set.
fn volume(ui: &mut Ui, playback: &Playback, width: f32) -> Option<SoundCommand> {
    let mut volume = playback.volume();
    let slider = egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false);
    let changed = ui.add_sized(vec2(width, 20.0), slider).changed();
    let glyph = if volume <= 0.001 {
        icon::SPEAKER_NONE
    } else if volume < 0.5 {
        icon::SPEAKER_LOW
    } else {
        icon::SPEAKER_HIGH
    };
    let name = playback.player.as_ref().map_or("", |player| player.device_name.as_str());
    ui.label(egui::RichText::new(glyph).size(theme::size::LARGE).color(theme::TEXT_WEAK)).on_hover_text(name);
    changed.then_some(SoundCommand::Volume(volume))
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
    ui.painter().text(
        rect.center() + nudge,
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(theme::size::LARGE),
        theme::ON_ACCENT,
    );
    response
}

/// A waveform to click or drag on to jump there, starting the sound if it isn't playing.
fn scrubber(ui: &mut Ui, height: f32, sound: &SoundArgs<'_>) -> Option<SoundCommand> {
    let now = sound.now();
    let length = sound.length(now.as_ref());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::CARD_RADIUS, theme::WELL);
    let progress = match (&now, length) {
        (Some(now), Some(total)) if total > 0.0 => Some((now.position / total) as f32),
        _ => None,
    };
    wave_or_note(&painter, rect, sound, progress);
    let inner = rect.shrink2(vec2(8.0, 0.0));
    let fraction_at = |x: f32| ((x - inner.left()) / inner.width()).clamp(0.0, 1.0);
    if let Some(progress) = progress {
        let x = inner.left() + progress * inner.width();
        painter
            .line_segment([pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)], Stroke::new(2.0, theme::PLAYHEAD));
    }
    if let Some(pointer) = response.interact_pointer_pos().or(response.hover_pos()) {
        let fraction = fraction_at(pointer.x);
        jump_mark(&painter, rect, inner.left() + fraction * inner.width(), length.map(|total| fraction as f64 * total));
    }
    let (Some(pointer), Some(total)) = (response.interact_pointer_pos(), length) else { return None };
    (response.clicked() || response.drag_stopped())
        .then(|| SoundCommand::PlayFrom(sound.index, fraction_at(pointer.x) as f64 * total))
}

/// The sound's waveform, or why there isn't one yet.
fn wave_or_note(painter: &egui::Painter, rect: Rect, sound: &SoundArgs<'_>, progress: Option<f32>) {
    let color = theme::kind_color(sound.library.entry(sound.index).kind);
    if let Some(waveform) = sound.waveform {
        let colors = [color.gamma_multiply(0.45), color];
        draw_waveform(painter, rect.shrink2(vec2(8.0, 10.0)), waveform, colors, progress);
    } else {
        let text = if sound.failed { "Couldn't read this sound" } else { "Reading…" };
        let font = FontId::proportional(theme::size::BODY);
        painter.text(rect.center(), Align2::CENTER_CENTER, text, font, theme::TEXT_FAINT);
    }
}

/// Where a click would jump to, at `x`, with the time there when the sound's length is known.
fn jump_mark(painter: &egui::Painter, rect: Rect, x: f32, time: Option<f64>) {
    let line = Stroke::new(1.0, theme::TEXT.gamma_multiply(0.4));
    painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], line);
    if let Some(time) = time {
        painter.text(
            pos2(x + 4.0, rect.top() + 4.0),
            Align2::LEFT_TOP,
            format_time(time),
            FontId::proportional(theme::size::FINE),
            theme::TEXT_WEAK,
        );
    }
}

/// Draws a waveform as one mesh: a bar per column from the lowest to the highest sample in it,
/// in `colors[0]`, and in `colors[1]` before `progress` (0..1), what's been played.
pub fn draw_waveform(
    painter: &egui::Painter,
    rect: Rect,
    waveform: &Waveform,
    colors: [Color32; 2],
    progress: Option<f32>,
) {
    let [color, played] = colors;
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
