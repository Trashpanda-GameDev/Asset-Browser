//! What's playing: the sound player and the sounds read into memory ahead of the selection, the
//! entry last played, and how sounds play (on their own when selected, looped, how loud).

use std::collections::HashMap;

use crate::asset::AssetKind;
use crate::audio::{NowPlaying, Player, ReadAhead, Source};
use crate::diag;
use crate::library::Library;
use crate::work::Wake;

/// Sounds read into memory ahead of the selection, in the direction of travel.
const READ_AHEAD_SOUNDS: usize = 3;

/// How loud sounds play until the user changes it.
pub const DEFAULT_VOLUME: f32 = 0.8;

pub struct Playback {
    pub player: Option<Player>,
    read_ahead: ReadAhead,
    /// The entry last sent to the player: (index, key).
    pub playing: Option<(u32, i64)>,
    pub autoplay: bool,
    looping: bool,
    volume: f32,
}

impl Playback {
    /// `wake` runs when the player has news, e.g. to draw the playhead again.
    pub fn start(wake: Wake) -> Self {
        let player = Player::start(wake).map_err(|err| diag::report("No sound output", err)).ok();
        let mut playback = Self {
            player,
            read_ahead: ReadAhead::start(),
            playing: None,
            autoplay: true,
            looping: false,
            volume: DEFAULT_VOLUME,
        };
        playback.set_volume(DEFAULT_VOLUME);
        playback
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
        let entry = library.entry(index);
        let source = match self.read_ahead.get(entry.key) {
            Some(bytes) => Source::Memory(bytes),
            None => Source::File(library.path_of(entry)),
        };
        player.play(entry.key, source, entry.extension());
        self.playing = Some((index, entry.key));
    }

    /// Plays `index` from `seconds` in, or jumps there if it's already playing.
    pub fn play_from(&mut self, library: &Library, index: u32, seconds: f64) {
        let key = library.entry(index).key;
        if self.now_for(key).is_none() {
            self.play(library, index);
        }
        if let Some(player) = &self.player {
            player.seek(seconds);
        }
    }

    /// Pauses or resumes `index`, or starts it if it isn't the one playing.
    pub fn toggle(&mut self, library: &Library, index: u32) {
        let key = library.entry(index).key;
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

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if let Some(player) = &self.player {
            player.set_volume(volume);
        }
    }

    /// Sounds start over when they end.
    pub fn looping(&self) -> bool {
        self.looping
    }

    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
        if let Some(player) = &self.player {
            player.set_looping(looping);
        }
    }

    /// Reads the sounds after `index` in `view` (and one before) into memory, so moving to them
    /// starts without a disk wait.
    pub fn read_ahead_around(&self, library: &Library, view: &[u32], index: u32) {
        let Some(position) = view.iter().position(|&i| i == index) else { return };
        let is_sound = |&&i: &&u32| library.entry(i).kind == AssetKind::Audio;
        let after = view[position + 1..].iter().filter(is_sound).take(READ_AHEAD_SOUNDS);
        let before = view[..position].iter().rev().filter(is_sound).take(1);
        let files = after
            .chain(before)
            .map(|&i| {
                let entry = library.entry(i);
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
