//! Sound effects and music, synthesised, as WAV files.

use std::f32::consts::{PI, TAU};
use std::fs;
use std::io;
use std::path::Path;

const RATE: u32 = 44_100;

#[expect(clippy::too_many_lines, reason = "refactor: split into named steps")]
pub(super) fn sounds(dir: &Path) -> io::Result<()> {
    let (sfx, music) = (dir.join("SFX"), dir.join("Music"));
    fs::create_dir_all(&sfx)?;
    fs::create_dir_all(&music)?;
    let mut noise = Noise(0x9e37_79b9);

    let mut phase = 0.0;
    wav(
        &sfx.join("jump.wav"),
        synth(0.32, |t| {
            phase += (260.0 * (3.0f32).powf(t / 0.32)) / RATE as f32;
            square(phase) * (1.0 - t / 0.32).powi(2)
        }),
    )?;
    wav(
        &sfx.join("coin.wav"),
        synth(0.5, |t| if t < 0.08 { square(t * 988.0) * 0.8 } else { square(t * 1319.0) * (-(t - 0.08) * 7.0).exp() }),
    )?;
    let mut phase = 0.0;
    wav(
        &sfx.join("hit.wav"),
        synth(0.3, |t| {
            phase += (140.0 - 80.0 * t / 0.3) / RATE as f32;
            noise.sample() * (-t * 28.0).exp() + sine(phase) * (-t * 12.0).exp() * 0.8
        }),
    )?;
    let (mut low, mut phase) = (0.0, 0.0);
    wav(
        &sfx.join("explosion.wav"),
        synth(1.7, |t| {
            low += (noise.sample() - low) * (0.3 * (-t * 2.0).exp() + 0.02);
            phase += 45.0 / RATE as f32;
            (low * 2.5 + sine(phase) * 0.5 * (-t * 3.0).exp()) * (t / 0.01).min(1.0) * (-t * 2.2).exp()
        }),
    )?;
    let mut phase = 0.0;
    wav(
        &sfx.join("laser.wav"),
        synth(0.36, |t| {
            phase += (1800.0 * (220.0f32 / 1800.0).powf(t / 0.36) * (1.0 + 0.02 * (t * 90.0).sin())) / RATE as f32;
            sine(phase) * (1.0 - t / 0.36)
        }),
    )?;
    wav(
        &sfx.join("powerup.wav"),
        synth(0.6, |t| {
            let notes = [523.25, 659.25, 783.99, 1046.5, 1318.5, 1568.0];
            let step = ((t / 0.1) as usize).min(notes.len() - 1);
            square(t * notes[step]) * (-(t % 0.1) * 12.0).exp() * (1.0 - t / 0.6 * 0.5)
        }),
    )?;
    let mut high = 0.0;
    wav(
        &sfx.join("footstep_grass.wav"),
        synth(1.7, |t| {
            let n = noise.sample();
            high += (n - high) * 0.35;
            let crunch: f32 = [0.1, 0.5, 0.9, 1.3]
                .iter()
                .map(|&at| {
                    let dt = t - at;
                    if (0.0..0.14).contains(&dt) {
                        (-dt * 32.0).exp() * (0.6 + 0.4 * (dt * 140.0).sin().abs())
                    } else {
                        0.0
                    }
                })
                .sum();
            (n - high) * crunch
        }),
    )?;
    let mut phase = 0.0;
    wav(
        &sfx.join("door_open.wav"),
        synth(1.5, |t| {
            phase += (105.0 + 30.0 * (t * 7.0).sin() + 6.0 * noise.sample()) / RATE as f32;
            let creak = saw(phase) * (t / 0.15).min(1.0) * (1.0 - (t - 0.9).max(0.0) / 0.3).max(0.0) * 0.5;
            let dt = t - 1.22;
            let thud = if dt >= 0.0 { sine(dt * 68.0) * (-dt * 14.0).exp() } else { 0.0 };
            creak + thud
        }),
    )?;
    wav(
        &sfx.join("click.wav"),
        synth(0.06, |t| sine(t * 2100.0) * (-t * 110.0).exp() + noise.sample() * (-t * 400.0).exp() * 0.5),
    )?;
    wav(
        &sfx.join("magic_spell.wav"),
        synth(1.4, |t| {
            let rise = 1.0 + t * 0.45;
            let tones: f32 = [880.0, 1320.0, 1760.0, 2640.0]
                .iter()
                .enumerate()
                .map(|(i, &f)| sine(t * f * rise) * (1.0 + (t * (9.0 + i as f32 * 3.0)).sin()) * 0.25)
                .sum();
            (tones + noise.sample() * 0.15 * (t * 23.0).sin().abs()) * (t / 0.12).min(1.0) * (-(t * 1.8)).exp()
        }),
    )?;

    wav(&music.join("menu_theme.wav"), menu_theme())?;
    wav(&music.join("battle_loop.wav"), battle_loop(&mut noise))?;
    wav(&music.join("ambience_forest.wav"), ambience(&mut noise))
}

fn menu_theme() -> Vec<f32> {
    let chords = [[261.63, 329.63, 392.0], [220.0, 261.63, 329.63], [174.61, 220.0, 261.63], [196.0, 246.94, 293.66]];
    let melody = [
        523.25, 0.0, 659.25, 587.33, 523.25, 0.0, 392.0, 0.0, 440.0, 523.25, 659.25, 0.0, 587.33, 0.0, 523.25, 0.0,
        349.23, 440.0, 523.25, 0.0, 587.33, 523.25, 440.0, 0.0, 392.0, 0.0, 493.88, 587.33, 783.99, 0.0, 587.33, 0.0,
    ];
    synth(16.0, |t| {
        let chord = chords[(t / 2.0) as usize % 4];
        let in_chord = t % 2.0;
        let pad_env = (in_chord / 0.3).min(1.0) * (1.0 - (in_chord - 1.6).max(0.0) / 0.4);
        let pad = chord.iter().map(|&f| tri(t * f)).sum::<f32>() / 3.0 * 0.35 * pad_env;
        let bass = sine(t * chord[0] / 2.0) * (-(t % 0.6) * 4.0).exp() * 0.5;
        let note = melody[(t / 0.25) as usize % melody.len()];
        let lead = if note > 0.0 { square(t * note) * 0.12 * (-(t % 0.25) * 6.0).exp() } else { 0.0 };
        (pad + bass + lead) * (t / 0.5).min(1.0) * ((16.0 - t) / 1.0).min(1.0)
    })
}

fn battle_loop(noise: &mut Noise) -> Vec<f32> {
    let bass_notes = [110.0, 110.0, 130.81, 110.0, 98.0, 110.0, 82.41, 98.0];
    let mut low = 0.0;
    synth(12.0, |t| {
        let beat = t % 0.4;
        let in_bar = t % 1.6;
        let kick = (TAU * (48.0 + 90.0 * (-beat * 25.0).exp()) * beat).sin() * (-beat * 9.0).exp();
        let snare_at = in_bar % 0.8 - 0.4;
        let snare = if snare_at >= 0.0 { noise.sample() * (-snare_at * 18.0).exp() * 0.55 } else { 0.0 };
        let hat = noise.sample() * (-(t % 0.2) * 60.0).exp() * 0.12;
        let raw = saw(t * bass_notes[(t / 0.2) as usize % 8]) * (-(t % 0.2) * 5.0).exp();
        low += (raw - low) * 0.12;
        let stab = [220.0, 261.63, 329.63].iter().map(|&f| square(t * f)).sum::<f32>() * (-in_bar * 5.0).exp() * 0.05;
        kick * 0.9 + snare + hat + low * 0.9 + stab
    })
}

fn ambience(noise: &mut Noise) -> Vec<f32> {
    let chirps = [
        (1.2, 3200.0),
        (1.45, 3600.0),
        (4.0, 2800.0),
        (4.2, 3000.0),
        (7.5, 3900.0),
        (7.7, 4100.0),
        (7.9, 3900.0),
        (11.0, 3000.0),
        (13.3, 3500.0),
        (13.5, 3700.0),
        (16.8, 2900.0),
        (17.0, 3100.0),
        (18.6, 4000.0),
    ];
    let mut low = 0.0;
    synth(20.0, |t| {
        low += (noise.sample() - low) * 0.02;
        let wind = low * (0.55 + 0.45 * (t * 0.35).sin()) * 3.0;
        let birds: f32 = chirps
            .iter()
            .map(|&(at, f)| {
                let dt = t - at;
                if (0.0..0.18).contains(&dt) {
                    sine(f * dt + 4000.0 * dt * dt) * (dt * PI / 0.18).sin() * 0.35
                } else {
                    0.0
                }
            })
            .sum();
        wind + birds
    })
}

struct Noise(u32);

impl Noise {
    /// From -1 to 1.
    fn sample(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

fn sine(phase: f32) -> f32 {
    (phase * TAU).sin()
}

fn square(phase: f32) -> f32 {
    if phase.fract() < 0.5 { 1.0 } else { -1.0 }
}

fn tri(phase: f32) -> f32 {
    4.0 * (phase.fract() - 0.5).abs() - 1.0
}

fn saw(phase: f32) -> f32 {
    2.0 * phase.fract() - 1.0
}

/// `seconds` of sound from `f(t)`, scaled so its loudest point is at 0.9.
fn synth(seconds: f32, mut f: impl FnMut(f32) -> f32) -> Vec<f32> {
    let mut samples: Vec<f32> = (0..(seconds * RATE as f32) as usize).map(|i| f(i as f32 / RATE as f32)).collect();
    let peak = samples.iter().fold(1e-6f32, |peak, v| peak.max(v.abs()));
    for sample in &mut samples {
        *sample *= 0.9 / peak;
    }
    samples
}

/// 16-bit mono WAV.
fn wav(path: &Path, samples: Vec<f32>) -> io::Result<()> {
    let data: Vec<u8> =
        samples.into_iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // one channel
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    fs::write(path, out)
}
