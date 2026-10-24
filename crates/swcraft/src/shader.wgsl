struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    base_color: vec4<f32>,
    edge_color: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    // x: ortho flag
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) m3: vec4<f32>,
    @location(6) tint: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) n_view: vec3<f32>,
    @location(1) p_view: vec3<f32>,
    @location(2) tint: vec4<f32>,
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
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

@fragment
fn fs_mesh(i: VsOut, @builtin(front_facing) ff: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.n_view);
    var v = normalize(-i.p_view);
    if (g.params.x > 0.5) {
        v = vec3<f32>(0.0, 0.0, 1.0);
    }
    // two-sided lighting: face the viewer
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let base = srgb_to_linear(mix(g.base_color.rgb, i.tint.rgb, i.tint.a));
    // key light (upper left, slightly toward the viewer), fill and rim, in view space
    let l1 = normalize(vec3<f32>(-0.45, 0.65, 0.62));
    let l2 = normalize(vec3<f32>(0.6, -0.25, 0.75));
    let d1 = max(dot(n, l1), 0.0);
    let d2 = max(dot(n, l2), 0.0);
    let hemi = 0.5 + 0.5 * n.y;
    let ambient = mix(vec3<f32>(0.16, 0.16, 0.18), vec3<f32>(0.30, 0.32, 0.36), hemi);
    let h = normalize(l1 + v);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * 0.35;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.12;
    let col = base * (ambient + 0.85 * d1 + 0.30 * d2) + vec3<f32>(spec + rim);
    return vec4<f32>(col, 1.0);
}

struct EdgeOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_edge(v: VsIn) -> EdgeOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    var o: EdgeOut;
    o.clip = g.view_proj * (model * vec4<f32>(v.pos, 1.0));
    return o;
}

@fragment
fn fs_edge() -> @location(0) vec4<f32> {
    return vec4<f32>(srgb_to_linear(g.edge_color.rgb), g.edge_color.a);
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
