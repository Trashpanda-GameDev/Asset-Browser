//! The 3D view's shader (`model.wgsl`), with the lighting it shares with the thumbnails filled in.

use crate::model::shading;
use crate::model::vector::normalize;

/// The WGSL (`model.wgsl`), with the lighting shared with the thumbnails filled in.
pub(super) fn shader_source() -> String {
    let v = |c: [f32; 3]| format!("vec3<f32>({:?}, {:?}, {:?})", c[0], c[1], c[2]);
    let n = |c: [f32; 3]| v(normalize(c));
    let values = [
        ("$KEY_DIR", n(shading::KEY_DIR)),
        ("$KEY_COLOR", v(shading::KEY_COLOR)),
        ("$FILL_DIR", n(shading::FILL_DIR)),
        ("$FILL_COLOR", v(shading::FILL_COLOR)),
        ("$RIM_DIR", n(shading::RIM_DIR)),
        ("$RIM_COLOR", v(shading::RIM_COLOR)),
        ("$SKY", v(shading::SKY)),
        ("$GROUND", v(shading::GROUND)),
        ("$SPECULAR", format!("{:?}", shading::SPECULAR)),
        ("$SHININESS", format!("{:?}", shading::SHININESS)),
    ];
    values.iter().fold(include_str!("model.wgsl").to_owned(), |source, (name, value)| source.replace(name, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks the WGSL without a GPU: it parses and passes naga's validation.
    #[test]
    fn shader_is_valid() {
        let module = naga::front::wgsl::parse_str(&shader_source()).expect("WGSL parses");
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("WGSL validates");
        assert!(module.entry_points.iter().any(|entry| entry.name == "fs_main"));
    }
}
