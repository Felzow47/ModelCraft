struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    base_color: vec4<f32>,
    edge_color: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    // x: ortho flag, y: section enabled, z: viewport width px, w: viewport height px
    params: vec4<f32>,
    // section plane: xyz normal, w offset (fragments with dot(p, n) > w are cut away)
    clip: vec4<f32>,
    // x key, y fill, z ambient, w specular
    light: vec4<f32>,
    // x rim, y flat shading flag, z hemisphere contrast, w shininess
    light2: vec4<f32>,
    // section cap colour
    cap: vec4<f32>,
    // camera position (world)
    eye: vec4<f32>,
    // xyz: unit vector toward the camera, w: how far edges are pulled toward it (world units)
    fwd: vec4<f32>,
    // x: headlight
    light3: vec4<f32>,
};

struct Pass {
    // pixel offset of this edge pass
    offset: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<uniform> pass_: Pass;
// diffuse texture of the mesh (a white texel when it has none), sRGB-encoded values
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var tex_s: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) m3: vec4<f32>,
    @location(6) tint: vec4<f32>,
    @location(7) color: vec4<f32>,
    // x: opacity
    @location(8) extra: vec4<f32>,
    @location(9) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) n_view: vec3<f32>,
    @location(1) p_view: vec3<f32>,
    @location(2) tint: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) p_world: vec3<f32>,
    @location(5) opacity: f32,
    @location(6) uv: vec2<f32>,
    @location(7) textured: f32,
};

@vertex
fn vs_mesh(v: VsIn) -> VsOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.pos, 1.0);
    let mv = g.view * model;
    var o: VsOut;
    o.clip = g.view_proj * world;
    o.n_view = (mv * vec4<f32>(v.nrm, 0.0)).xyz;
    o.p_view = (g.view * world).xyz;
    o.tint = v.tint;
    o.color = v.color;
    o.uv = v.uv;
    o.textured = v.extra.y;
    o.p_world = world.xyz;
    o.opacity = v.extra.x;
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

fn cut(p: vec3<f32>) -> bool {
    return g.params.y > 0.5 && dot(p, g.clip.xyz) > g.clip.w;
}

@fragment
fn fs_mesh(i: VsOut, @builtin(front_facing) ff: bool) -> @location(0) vec4<f32> {
    // sampled first: implicit derivatives need uniform control flow
    let texel = select(vec4<f32>(1.0), textureSample(tex, tex_s, i.uv), i.textured > 0.5);
    if (cut(i.p_world)) {
        discard;
    }
    var n = normalize(i.n_view);
    var v = normalize(-i.p_view);
    if (g.params.x > 0.5) {
        v = vec3<f32>(0.0, 0.0, 1.0);
    }
    // cut-out textures (lettering or leaves on a transparent image)
    if (texel.a < 0.5) {
        discard;
    }
    let own = mix(g.base_color.rgb, i.color.rgb * texel.rgb, i.color.a);
    let base_srgb = mix(own, i.tint.rgb, i.tint.a);
    // inside of a sectioned solid: flat cap colour
    if (g.params.y > 0.5 && dot(n, v) < 0.0) {
        return vec4<f32>(srgb_to_linear(g.cap.rgb), i.opacity);
    }
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let base = srgb_to_linear(base_srgb);
    if (g.light2.y > 0.5) {
        let k = 0.78 + 0.22 * max(dot(n, v), 0.0);
        return vec4<f32>(base * k, i.opacity);
    }
    let l1 = normalize(vec3<f32>(-0.45, 0.65, 0.62));
    let l2 = normalize(vec3<f32>(0.6, -0.25, 0.75));
    let d1 = max(dot(n, l1), 0.0);
    let d2 = max(dot(n, l2), 0.0);
    let hemi = 0.5 + 0.5 * n.y;
    let head = max(dot(n, v), 0.0) * g.light3.x;
    let amb = g.light.z * mix(vec3<f32>(1.0 - g.light2.z), vec3<f32>(1.0 + g.light2.z), hemi);
    let h = normalize(l1 + v);
    let spec = pow(max(dot(n, h), 0.0), g.light2.w) * g.light.w;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * g.light2.x;
    var col = base * (amb + g.light.x * d1 + g.light.y * d2 + head) + vec3<f32>(spec + rim);
    var a = i.opacity;
    if (a < 0.999) {
        // see-through parts: brighter, glass-like silhouettes so the shape stays readable
        let f = pow(1.0 - max(dot(n, v), 0.0), 2.0);
        let t = 1.0 - a;
        col = col * (1.0 + 0.35 * t) + vec3<f32>(0.06 * t + 0.22 * f * t);
        a = mix(a, min(1.0, a + 0.4), f);
    }
    return vec4<f32>(col, a);
}

struct EdgeOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) p_world: vec3<f32>,
    @location(1) opacity: f32,
};

@vertex
fn vs_edge(v: VsIn) -> EdgeOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = model * vec4<f32>(v.pos, 1.0);
    var o: EdgeOut;
    // pull the edge toward the camera by a fixed amount so it never z-fights with its faces
    // (depth bias does not apply to lines). Perspective: along the eye ray, so it stays on the
    // same pixel; orthographic: along the view direction.
    var pushed = world.xyz + g.fwd.xyz * g.fwd.w;
    if (g.params.x < 0.5) {
        let to_eye = g.eye.xyz - world.xyz;
        let d = length(to_eye);
        pushed = world.xyz + to_eye * (min(g.fwd.w, d * 0.5) / max(d, 1e-20));
    }
    var c = g.view_proj * vec4<f32>(pushed, 1.0);
    let px = vec2<f32>(2.0 / max(g.params.z, 1.0), 2.0 / max(g.params.w, 1.0));
    c = vec4<f32>(c.xy + pass_.offset.xy * px * c.w, c.z, c.w);
    o.clip = c;
    o.p_world = world.xyz;
    o.opacity = v.extra.x;
    return o;
}

@fragment
fn fs_edge(i: EdgeOut) -> @location(0) vec4<f32> {
    if (cut(i.p_world)) {
        discard;
    }
    return vec4<f32>(srgb_to_linear(g.edge_color.rgb), g.edge_color.a * i.opacity);
}

// Coloured world-space lines (grid, bounding box, section outline).
struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) col: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) col: vec4<f32>,
};

@vertex
fn vs_line(v: LineIn) -> LineOut {
    var o: LineOut;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.col = v.col;
    return o;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    return vec4<f32>(srgb_to_linear(i.col.rgb), i.col.a);
}

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
};

@vertex
fn vs_bg(@builtin(vertex_index) vi: u32) -> BgOut {
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    var o: BgOut;
    o.clip = vec4<f32>(x, y, 1.0, 1.0);
    o.t = y * 0.5 + 0.5;
    return o;
}

@fragment
fn fs_bg(i: BgOut) -> @location(0) vec4<f32> {
    let c = mix(g.bg_bottom.rgb, g.bg_top.rgb, i.t);
    return vec4<f32>(srgb_to_linear(c), 1.0);
}
