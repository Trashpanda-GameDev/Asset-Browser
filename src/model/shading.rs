//! The lighting shared by both model renderers, so the CPU thumbnails (`raster`) and the GPU
//! viewer look the same: the viewer's shader is generated from these constants.
//!
//! Conventions:
//! - Model space is right-handed, Y up, with the model's front facing +Z.
//! - View space: the camera looks down -Z, +Y is up, +X is right. Every direction here is in view
//!   space, points toward the light, and has unit length. The lights are fixed to the camera, so a
//!   model is lit the same from whichever side it's seen.
//! - Colours are linear RGB.
//! - Double-sided: a back-facing fragment (clockwise on screen) is shaded with its normal flipped.
//!
//! A soft studio rig: a warm key light from the upper left in front, a cooler and weaker fill
//! from the right, a rim from behind and above to separate the silhouette, and a hemisphere
//! ambient (sky above, ground below) that keeps the shadowed side readable.

pub const KEY_DIR: [f32; 3] = [-0.501_406, 0.601_687, 0.621_743];
pub const KEY_COLOR: [f32; 3] = [0.92, 0.85, 0.74];
pub const FILL_DIR: [f32; 3] = [0.857_537, 0.100_887, 0.504_433];
pub const FILL_COLOR: [f32; 3] = [0.25, 0.3, 0.38];
pub const RIM_DIR: [f32; 3] = [0.251_896, 0.604_551, -0.755_689];
pub const RIM_COLOR: [f32; 3] = [0.4, 0.42, 0.48];
/// Hemisphere ambient, mixed by `normal.y * 0.5 + 0.5`: GROUND for normals pointing down, SKY up.
pub const SKY: [f32; 3] = [0.3, 0.32, 0.38];
pub const GROUND: [f32; 3] = [0.15, 0.13, 0.12];
/// Blinn-Phong highlight of the key light only, with the view vector V = (0, 0, 1).
pub const SPECULAR: f32 = 0.18;
pub const SHININESS: f32 = 24.0;
/// Linear "clay" albedo for surfaces without a material.
pub const DEFAULT_ALBEDO: [f32; 3] = [0.55, 0.55, 0.57];
/// Default camera direction, radians: `view_direction(VIEW_YAW, VIEW_PITCH)`. In front of the
/// model, to its right and slightly above, as product shots are taken.
pub const VIEW_YAW: f32 = 0.610_865_2; // 35°
pub const VIEW_PITCH: f32 = 0.349_065_9; // 20°
/// Vertical field of view, radians. Narrow, for a product-shot look with little distortion.
pub const FOV_Y: f32 = std::f32::consts::FRAC_PI_6; // 30°

/// normalize(KEY_DIR + V), the key light's Blinn-Phong half vector.
const KEY_HALF: [f32; 3] = [-0.278_409, 0.334_091, 0.900_484];

/// Unit vector from the model toward the camera, in model space. `yaw` turns from the front (+Z)
/// toward the right (+X); `pitch` raises the camera above the horizon.
pub fn view_direction(yaw: f32, pitch: f32) -> [f32; 3] {
    [yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()]
}

/// Lights a surface point: `albedo` and the result are linear RGB, `normal_view` is a unit
/// normal in view space, already flipped for back faces.
///
/// albedo × (mix(GROUND, SKY, n.y × 0.5 + 0.5) + Σ COLORᵢ × max(n · DIRᵢ, 0))
///   + SPECULAR × KEY_COLOR × max(n · normalize(KEY_DIR + V), 0)^SHININESS
pub fn shade(albedo: [f32; 3], normal_view: [f32; 3]) -> [f32; 3] {
    let n = normal_view;
    let dot = |d: [f32; 3]| (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]).max(0.0);
    let up = n[1] * 0.5 + 0.5;
    let (key, fill, rim) = (dot(KEY_DIR), dot(FILL_DIR), dot(RIM_DIR));
    let half = dot(KEY_HALF);
    // Below 0.75 the highlight is under 1/1000 and invisible; skipping `powf` there saves most
    // of its cost on the CPU.
    let highlight = if half > 0.75 { SPECULAR * half.powf(SHININESS) } else { 0.0 };
    [0, 1, 2].map(|c| {
        let ambient = GROUND[c] + (SKY[c] - GROUND[c]) * up;
        let light = ambient + KEY_COLOR[c] * key + FILL_COLOR[c] * fill + RIM_COLOR[c] * rim;
        albedo[c] * light + highlight * KEY_COLOR[c]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn length(v: [f32; 3]) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    #[test]
    fn directions_are_unit_length() {
        for dir in [KEY_DIR, FILL_DIR, RIM_DIR, KEY_HALF, view_direction(VIEW_YAW, VIEW_PITCH)] {
            assert!((length(dir) - 1.0).abs() < 1e-5, "{dir:?}");
        }
        let half = [KEY_DIR[0], KEY_DIR[1], KEY_DIR[2] + 1.0];
        let half = half.map(|c| c / length(half));
        for (a, b) in half.iter().zip(KEY_HALF) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn default_view_is_front_right_above() {
        let [x, y, z] = view_direction(VIEW_YAW, VIEW_PITCH);
        assert!(x > 0.0 && y > 0.0 && z > x && z > y);
    }

    #[test]
    fn lit_side_is_brighter_and_nothing_blows_out() {
        let toward_key = shade([1.0; 3], KEY_DIR);
        let away = shade([1.0; 3], KEY_DIR.map(|c| -c));
        assert!(toward_key[1] > away[1] * 2.0);
        // White facing the key light stays close to, but within, the displayable range.
        assert!(toward_key.iter().all(|&c| c > 0.8 && c < 1.35), "{toward_key:?}");
    }
}
