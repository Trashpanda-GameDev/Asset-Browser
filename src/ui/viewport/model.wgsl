// The 3D view's shader. The `$NAME`s are the lighting the thumbnails share (`model::shading`),
// filled in by `shader_source`.

const KEY_DIR = $KEY_DIR;
const KEY_COLOR = $KEY_COLOR;
const FILL_DIR = $FILL_DIR;
const FILL_COLOR = $FILL_COLOR;
const RIM_DIR = $RIM_DIR;
const RIM_COLOR = $RIM_COLOR;
const SKY = $SKY;
const GROUND = $GROUND;
const SPECULAR = $SPECULAR;
const SHININESS = $SHININESS;

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    line_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct Material {
    color: vec4<f32>,
    flags: vec4<f32>,
};
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var base_texture: texture_2d<f32>;
@group(1) @binding(2) var base_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOut {
    // Invariant: the front and back passes must put a shared position at exactly the same depth.
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.normal = (globals.view * vec4<f32>(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

fn shade(albedo: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    var light = mix(GROUND, SKY, n.y * 0.5 + 0.5);
    light += KEY_COLOR * max(dot(n, KEY_DIR), 0.0);
    light += FILL_COLOR * max(dot(n, FILL_DIR), 0.0);
    light += RIM_COLOR * max(dot(n, RIM_DIR), 0.0);
    let half_vector = normalize(KEY_DIR + vec3<f32>(0.0, 0.0, 1.0));
    let specular = SPECULAR * KEY_COLOR * pow(max(dot(n, half_vector), 0.0), SHININESS);
    return albedo * light + specular;
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let base = material.color * in.color * textureSample(base_texture, base_sampler, in.uv);
    if (base.a < material.flags.x) {
        discard;
    }
    // Both sides are lit: a back face (clockwise on screen) is shaded with its normal flipped. A
    // missing normal faces the camera.
    var n = vec3<f32>(0.0, 0.0, 1.0);
    if (dot(in.normal, in.normal) > 1e-12) {
        n = normalize(in.normal);
        if (!front) {
            n = -n;
        }
    }
    return vec4<f32>(shade(base.rgb, n), 1.0);
}

@vertex
fn vs_line(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    var clip = globals.view_proj * vec4<f32>(position, 1.0);
    // Pulled slightly toward the camera so edges win over the surface they lie on.
    clip.z = clip.z - 0.0006 * clip.w;
    return clip;
}

@fragment
fn fs_line() -> @location(0) vec4<f32> {
    return globals.line_color;
}
