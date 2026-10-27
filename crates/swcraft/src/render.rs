//! wgpu renderer: draws a document's scene into an offscreen (MSAA) texture that egui displays.
//!
//! One `Renderer` (pipelines + offscreen target) is shared by every document; each document
//! owns its `GpuScene` (vertex/index buffers and per-component instances).

use bytemuck::{Pod, Zeroable};
use eframe::wgpu;
use glam::Mat4;
use sw_format::Scene;
use wgpu::util::DeviceExt;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Texture shown by egui (raw sRGB bytes copied from the render target).
const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Uniform slots for the edge passes (thick edges are drawn several times, offset by pixels).
const PASS_SLOTS: usize = 9;
const PASS_STRIDE: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub nrm: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    model: [f32; 16],
    tint: [f32; 4],
    color: [f32; 4],
    /// x: opacity
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub col: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [f32; 16],
    view: [f32; 16],
    base_color: [f32; 4],
    edge_color: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    params: [f32; 4],
    clip: [f32; 4],
    light: [f32; 4],
    light2: [f32; 4],
    cap: [f32; 4],
    eye: [f32; 4],
    fwd: [f32; 4],
    light3: [f32; 4],
}

/// Largest single GPU buffer we create. wgpu's default `max_buffer_size` is 256 MiB and many
/// adapters refuse more, so a large scene is split into several vertex/index buffer chunks.
const CHUNK_BYTES: u64 = 128 << 20;

/// Part of one mesh stored in one chunk.
#[derive(Clone, Copy, Default)]
struct Piece {
    chunk: u32,
    base_vertex: i32,
    tri_start: u32,
    tri_count: u32,
    edge_start: u32,
    edge_count: u32,
}

struct Chunk {
    vbuf: wgpu::Buffer,
    tri_ibuf: Option<wgpu::Buffer>,
    edge_ibuf: Option<wgpu::Buffer>,
}

/// One drawable instance of a mesh.
pub struct InstanceDesc {
    pub mesh: usize,
    pub model: [f32; 16],
    /// Selection / hover tint (alpha = strength).
    pub tint: [f32; 4],
    /// Own colour from the file (alpha 0 = use the base colour).
    pub color: [f32; 4],
    /// 1 = opaque; below 1 the instance is drawn in the sorted transparent pass.
    pub opacity: f32,
    /// World-space centre (for back-to-front sorting of transparent instances).
    pub center: [f32; 3],
}

/// GPU copy of a loaded scene.
pub struct GpuScene {
    chunks: Vec<Chunk>,
    /// Pieces of each mesh (usually one).
    pieces: Vec<Vec<Piece>>,
    inst_buf: Option<wgpu::Buffer>,
    /// Opaque batches: (mesh index, first instance, instance count)
    batches: Vec<(usize, u32, u32)>,
    /// Transparent instances: (mesh, instance index, world centre)
    transparent: Vec<(usize, u32, [f32; 3])>,
    /// Highlighted triangles (already in world space): vertices, instance, index count.
    hl: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    pub gpu_bytes: u64,
}

/// CPU-side chunk under construction.
#[derive(Default)]
struct ChunkBuild {
    verts: Vec<Vertex>,
    tris: Vec<u32>,
    edges: Vec<u32>,
}

impl GpuScene {
    pub fn upload(device: &wgpu::Device, scene: &Scene) -> GpuScene {
        // MODELCRAFT_GPU_CHUNK_KB forces small chunks (tests the splitting path)
        let chunk = std::env::var("MODELCRAFT_GPU_CHUNK_KB").ok().and_then(|v| v.parse::<u64>().ok()).map(|k| k << 10).unwrap_or(CHUNK_BYTES);
        let max = device.limits().max_buffer_size.min(chunk).max(4096);
        let vmax = (max / std::mem::size_of::<Vertex>() as u64) as usize;
        let imax = (max / 4) as usize;
        let mut gs = GpuScene { chunks: Vec::new(), pieces: Vec::with_capacity(scene.meshes.len()), inst_buf: None, batches: Vec::new(), transparent: Vec::new(), hl: None, gpu_bytes: 0 };
        let mut cur = ChunkBuild::default();
        for m in &scene.meshes {
            let m = &m.mesh;
            let mut pieces = Vec::new();
            let nv = m.positions.len().min(m.normals.len());
            if nv == 0 || (m.indices.is_empty() && m.edges.is_empty()) {
                gs.pieces.push(pieces);
                continue;
            }
            let fits = |c: &ChunkBuild| c.verts.len() + nv <= vmax && c.tris.len() + m.indices.len() <= imax && c.edges.len() + m.edges.len() <= imax;
            if !fits(&cur) && !cur.verts.is_empty() {
                gs.flush(device, &mut cur);
            }
            if fits(&cur) {
                pieces.push(Piece {
                    chunk: gs.chunks.len() as u32,
                    base_vertex: cur.verts.len() as i32,
                    tri_start: cur.tris.len() as u32,
                    tri_count: m.indices.len() as u32,
                    edge_start: cur.edges.len() as u32,
                    edge_count: m.edges.len() as u32,
                });
                cur.verts.extend(m.positions[..nv].iter().zip(&m.normals[..nv]).map(|(p, n)| Vertex { pos: *p, nrm: *n }));
                cur.tris.extend(m.indices.iter().map(|&i| i.min(nv as u32 - 1)));
                cur.edges.extend(m.edges.iter().map(|&i| i.min(nv as u32 - 1)));
            } else {
                // a single mesh larger than a chunk: split triangles then edges into pieces
                gs.split(device, m, nv, &m.indices, 3, vmax, imax, &mut pieces);
                gs.split(device, m, nv, &m.edges, 2, vmax, imax, &mut pieces);
            }
            gs.pieces.push(pieces);
        }
        if !cur.verts.is_empty() || gs.chunks.is_empty() {
            if cur.verts.is_empty() {
                cur.verts.push(Vertex::zeroed());
            }
            gs.flush(device, &mut cur);
        }
        gs
    }

    fn flush(&mut self, device: &wgpu::Device, c: &mut ChunkBuild) {
        let mk = |label, bytes: &[u8], usage| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage });
        let vbuf = mk("vertices", bytemuck::cast_slice(&c.verts), wgpu::BufferUsages::VERTEX);
        let tri_ibuf = (!c.tris.is_empty()).then(|| mk("triangles", bytemuck::cast_slice(&c.tris), wgpu::BufferUsages::INDEX));
        let edge_ibuf = (!c.edges.is_empty()).then(|| mk("edges", bytemuck::cast_slice(&c.edges), wgpu::BufferUsages::INDEX));
        self.gpu_bytes += (c.verts.len() * std::mem::size_of::<Vertex>() + c.tris.len() * 4 + c.edges.len() * 4) as u64;
        self.chunks.push(Chunk { vbuf, tri_ibuf, edge_ibuf });
        *c = ChunkBuild::default();
    }

    /// Split the primitives of an oversized mesh into chunks with their own (remapped) vertices.
    #[allow(clippy::too_many_arguments)]
    fn split(&mut self, device: &wgpu::Device, m: &sw_format::TriMesh, nv: usize, idx: &[u32], k: usize, vmax: usize, imax: usize, out: &mut Vec<Piece>) {
        if idx.is_empty() {
            return;
        }
        let mut remap = vec![u32::MAX; nv];
        let mut touched: Vec<u32> = Vec::new();
        let mut c = ChunkBuild::default();
        let mut prims = idx.chunks_exact(k).peekable();
        while prims.peek().is_some() {
            while let Some(p) = prims.peek() {
                if c.verts.len() + k > vmax || c.tris.len() + c.edges.len() + k > imax {
                    break;
                }
                for &v in p.iter() {
                    let v = (v as usize).min(nv - 1);
                    if remap[v] == u32::MAX {
                        remap[v] = c.verts.len() as u32;
                        touched.push(v as u32);
                        c.verts.push(Vertex { pos: m.positions[v], nrm: m.normals[v] });
                    }
                    let local = remap[v];
                    if k == 3 { c.tris.push(local) } else { c.edges.push(local) }
                }
                prims.next();
            }
            let piece = Piece {
                chunk: self.chunks.len() as u32,
                base_vertex: 0,
                tri_start: 0,
                tri_count: c.tris.len() as u32,
                edge_start: 0,
                edge_count: c.edges.len() as u32,
            };
            self.flush(device, &mut c);
            out.push(piece);
            for v in touched.drain(..) {
                remap[v as usize] = u32::MAX;
            }
        }
    }

    pub fn set_instances(&mut self, device: &wgpu::Device, mut items: Vec<InstanceDesc>) {
        // opaque first (batched by mesh), then transparent ones (drawn one by one, sorted per frame)
        items.sort_by_key(|i| (i.opacity < 0.999, i.mesh));
        let mut batches: Vec<(usize, u32, u32)> = Vec::new();
        let mut transparent = Vec::new();
        for (k, it) in items.iter().enumerate() {
            if it.opacity < 0.999 {
                transparent.push((it.mesh, k as u32, it.center));
                continue;
            }
            match batches.last_mut() {
                Some(b) if b.0 == it.mesh => b.2 += 1,
                _ => batches.push((it.mesh, k as u32, 1)),
            }
        }
        let inst: Vec<Instance> = items
            .into_iter()
            .map(|i| Instance { model: i.model, tint: i.tint, color: i.color, extra: [i.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0] })
            .collect();
        self.inst_buf = (!inst.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instances"),
                contents: bytemuck::cast_slice(&inst),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        self.batches = batches;
        self.transparent = transparent;
    }

    /// Triangles to draw on top in the highlight colour (world space, 3 vertices per triangle).
    pub fn set_highlight(&mut self, device: &wgpu::Device, tris: &[Vertex], color: [f32; 4]) {
        if tris.is_empty() {
            self.hl = None;
            return;
        }
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("highlight"),
            contents: bytemuck::cast_slice(tris),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let inst = Instance { model: Mat4::IDENTITY.to_cols_array(), tint: color, color: [0.0; 4], extra: [1.0, 0.0, 0.0, 0.0] };
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("highlight inst"),
            contents: bytemuck::bytes_of(&inst),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.hl = Some((vb, ib, tris.len() as u32));
    }
}

#[derive(Clone, Copy)]
pub struct Light {
    pub key: f32,
    pub fill: f32,
    pub ambient: f32,
    pub spec: f32,
    pub rim: f32,
    pub flat: bool,
    pub hemi: f32,
    pub shininess: f32,
    /// Light from the viewer's position: no face seen head-on is ever dark.
    pub head: f32,
}

impl Light {
    pub fn preset(p: crate::settings::Lighting) -> Light {
        use crate::settings::Lighting::*;
        match p {
            Studio => Light { key: 0.72, fill: 0.34, ambient: 0.36, spec: 0.30, rim: 0.10, flat: false, hemi: 0.22, shininess: 48.0, head: 0.24 },
            Soft => Light { key: 0.50, fill: 0.38, ambient: 0.52, spec: 0.12, rim: 0.06, flat: false, hemi: 0.12, shininess: 24.0, head: 0.22 },
            Contrast => Light { key: 1.0, fill: 0.20, ambient: 0.20, spec: 0.50, rim: 0.18, flat: false, hemi: 0.4, shininess: 80.0, head: 0.10 },
            Flat => Light { key: 0.0, fill: 0.0, ambient: 1.0, spec: 0.0, rim: 0.0, flat: true, hemi: 0.0, shininess: 1.0, head: 0.0 },
        }
    }
}

pub struct ViewSettings {
    pub view_proj: Mat4,
    pub view: Mat4,
    pub ortho: bool,
    pub edges: bool,
    pub edge_width: f32,
    pub edge_color: [f32; 4],
    pub base_color: [f32; 3],
    pub bg_top: [f32; 3],
    pub bg_bottom: [f32; 3],
    pub light: Light,
    /// Section plane (normal, offset) when active.
    pub section: Option<([f32; 3], f32)>,
    pub cap_color: [f32; 3],
    /// Edges are drawn this far (world units) closer to the camera than the faces, so they
    /// never sink into the surface at some angles.
    pub edge_push: f32,
    /// Grid / bounding box lines (line list, world space).
    pub lines: Vec<LineVertex>,
}

pub struct Renderer {
    pub samples: u32,
    pipe_mesh: wgpu::RenderPipeline,
    pipe_tr_depth: wgpu::RenderPipeline,
    pipe_tr: wgpu::RenderPipeline,
    pipe_hl: wgpu::RenderPipeline,
    pipe_edge: wgpu::RenderPipeline,
    pipe_line: wgpu::RenderPipeline,
    pipe_bg: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    passes: wgpu::Buffer,
    bind: wgpu::BindGroup,
    target: Option<Target>,
}

pub struct Target {
    pub size: [u32; 2],
    msaa: Option<wgpu::TextureView>,
    pub resolve: wgpu::Texture,
    pub resolve_view: wgpu::TextureView,
    /// Same pixels in a non-sRGB texture: egui samples textures as gamma-encoded values, so
    /// handing it the sRGB texture directly would decode (darken) the image a second time.
    pub display: wgpu::Texture,
    pub display_view: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, samples: u32) -> Self {
        let samples = if [1, 2, 4, 8].contains(&samples) { samples } else { 4 };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("modelcraft shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let passes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("edge passes"),
            size: PASS_STRIDE * PASS_SLOTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &passes, offset: 0, size: wgpu::BufferSize::new(16) }),
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let iattrs = wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4];
        let lattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vattrs,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &iattrs,
            }),
        ];
        let line_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &lattrs,
        })];
        let ms = wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false };
        let target = [Some(wgpu::ColorTargetState {
            format: COLOR_FORMAT,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let target_none = [Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend: None, write_mask: wgpu::ColorWrites::empty() })];
        let depth = |write: bool, cmp: wgpu::CompareFunction, bias: wgpu::DepthBiasState| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(cmp),
                stencil: Default::default(),
                bias,
            })
        };
        let pipe = |label: &str,
                    vs: &str,
                    fs: &str,
                    bufs: &[Option<wgpu::VertexBufferLayout>],
                    topo: wgpu::PrimitiveTopology,
                    ds: Option<wgpu::DepthStencilState>,
                    color: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers: bufs },
                primitive: wgpu::PrimitiveState { topology: topo, cull_mode: None, ..Default::default() },
                depth_stencil: ds,
                multisample: ms,
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(), targets: if color { &target } else { &target_none } }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction as C;
        use wgpu::PrimitiveTopology as T;
        let pipe_mesh = pipe(
            "mesh",
            "vs_mesh",
            "fs_mesh",
            &buffers,
            T::TriangleList,
            depth(true, C::Less, wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 }),
            true,
        );
        // transparent instances: depth-only prepass per instance, then colour where depth is equal
        // (only the nearest layer of each transparent part is blended → clean "x-ray" look)
        let pipe_tr_depth = pipe(
            "transparent depth",
            "vs_mesh",
            "fs_mesh",
            &buffers,
            T::TriangleList,
            depth(true, C::Less, wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 }),
            false,
        );
        let pipe_tr = pipe(
            "transparent",
            "vs_mesh",
            "fs_mesh",
            &buffers,
            T::TriangleList,
            depth(false, C::LessEqual, wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 }),
            true,
        );
        let pipe_hl = pipe(
            "highlight",
            "vs_mesh",
            "fs_mesh",
            &buffers,
            T::TriangleList,
            depth(false, C::LessEqual, wgpu::DepthBiasState { constant: -1, slope_scale: -1.0, clamp: 0.0 }),
            true,
        );
        let pipe_edge = pipe("edges", "vs_edge", "fs_edge", &buffers, T::LineList, depth(false, C::LessEqual, Default::default()), true);
        let pipe_line = pipe("lines", "vs_line", "fs_line", &line_buffers, T::LineList, depth(false, C::LessEqual, Default::default()), true);
        let pipe_bg = pipe("background", "vs_bg", "fs_bg", &[], T::TriangleList, depth(false, C::Always, Default::default()), true);
        Renderer { samples, pipe_mesh, pipe_tr_depth, pipe_tr, pipe_hl, pipe_edge, pipe_line, pipe_bg, globals, passes, bind, target: None }
    }

    /// Ensure the offscreen target has the given size. Returns true if (re)created.
    pub fn ensure_target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        let ext = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let mk = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: ext,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let msaa = (self.samples > 1).then(|| mk("msaa", COLOR_FORMAT, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default()));
        let resolve = mk(
            "resolve",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = mk("depth", DEPTH_FORMAT, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let display = mk("display", DISPLAY_FORMAT, 1, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        self.target = Some(Target {
            size,
            msaa,
            resolve_view: resolve.create_view(&Default::default()),
            resolve,
            display_view: display.create_view(&Default::default()),
            display,
            depth: depth.create_view(&Default::default()),
        });
        true
    }

    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    pub fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, gs: Option<&GpuScene>, vs: &ViewSettings) {
        let Some(t) = &self.target else { return };
        let (sec_on, sec) = match vs.section {
            Some((n, d)) => (1.0, [n[0], n[1], n[2], d]),
            None => (0.0, [0.0, 1.0, 0.0, 0.0]),
        };
        let l = vs.light;
        let g = Globals {
            view_proj: vs.view_proj.to_cols_array(),
            view: vs.view.to_cols_array(),
            base_color: [vs.base_color[0], vs.base_color[1], vs.base_color[2], 1.0],
            edge_color: vs.edge_color,
            bg_top: [vs.bg_top[0], vs.bg_top[1], vs.bg_top[2], 1.0],
            bg_bottom: [vs.bg_bottom[0], vs.bg_bottom[1], vs.bg_bottom[2], 1.0],
            params: [if vs.ortho { 1.0 } else { 0.0 }, sec_on, t.size[0] as f32, t.size[1] as f32],
            clip: sec,
            light: [l.key, l.fill, l.ambient, l.spec],
            light2: [l.rim, if l.flat { 1.0 } else { 0.0 }, l.hemi, l.shininess],
            cap: [vs.cap_color[0], vs.cap_color[1], vs.cap_color[2], 1.0],
            eye: {
                let inv = vs.view.inverse();
                let e = inv.w_axis;
                [e.x, e.y, e.z, 0.0]
            },
            fwd: {
                let b = vs.view.inverse().z_axis.truncate().normalize_or_zero();
                [b.x, b.y, b.z, vs.edge_push]
            },
            light3: [l.head, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        // pixel offsets for thick edges: a small disc of passes
        let offsets = edge_offsets(vs.edge_width);
        let mut pass_data = vec![0u8; (PASS_STRIDE as usize) * PASS_SLOTS];
        for (k, o) in offsets.iter().enumerate() {
            let v = [o[0], o[1], 0.0f32, 0.0];
            pass_data[k * PASS_STRIDE as usize..k * PASS_STRIDE as usize + 16].copy_from_slice(bytemuck::bytes_of(&v));
        }
        queue.write_buffer(&self.passes, 0, &pass_data);
        let line_buf = (!vs.lines.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("lines"),
                contents: bytemuck::cast_slice(&vs.lines),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let (view, resolve_target) = match &t.msaa {
                Some(m) => (m, Some(&t.resolve_view)),
                None => (&t.resolve_view, None),
            };
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: if t.msaa.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &self.bind, &[0]);
            rp.set_pipeline(&self.pipe_bg);
            rp.draw(0..3, 0..1);
            if let Some(gs) = gs {
                if let Some(ib) = &gs.inst_buf {
                    rp.set_vertex_buffer(1, ib.slice(..));
                    // opaque
                    rp.set_pipeline(&self.pipe_mesh);
                    for (ci, ch) in gs.chunks.iter().enumerate() {
                        let Some(tb) = &ch.tri_ibuf else { continue };
                        rp.set_vertex_buffer(0, ch.vbuf.slice(..));
                        rp.set_index_buffer(tb.slice(..), wgpu::IndexFormat::Uint32);
                        for &(m, first, count) in &gs.batches {
                            for p in gs.pieces[m].iter().filter(|p| p.chunk as usize == ci && p.tri_count > 0) {
                                rp.draw_indexed(p.tri_start..p.tri_start + p.tri_count, p.base_vertex, first..first + count);
                            }
                        }
                    }
                    if vs.edges {
                        rp.set_pipeline(&self.pipe_edge);
                        for k in 0..offsets.len() {
                            rp.set_bind_group(0, &self.bind, &[(k as u64 * PASS_STRIDE) as u32]);
                            for (ci, ch) in gs.chunks.iter().enumerate() {
                                let Some(eb) = &ch.edge_ibuf else { continue };
                                rp.set_vertex_buffer(0, ch.vbuf.slice(..));
                                rp.set_index_buffer(eb.slice(..), wgpu::IndexFormat::Uint32);
                                for &(m, first, count) in &gs.batches {
                                    for p in gs.pieces[m].iter().filter(|p| p.chunk as usize == ci && p.edge_count > 0) {
                                        rp.draw_indexed(p.edge_start..p.edge_start + p.edge_count, p.base_vertex, first..first + count);
                                    }
                                }
                            }
                        }
                        rp.set_bind_group(0, &self.bind, &[0]);
                    }
                    // transparent, far to near
                    if !gs.transparent.is_empty() {
                        let mut order: Vec<(f32, usize, u32)> = gs
                            .transparent
                            .iter()
                            .map(|&(m, inst, c)| ((vs.view * glam::Vec3::from(c).extend(1.0)).z, m, inst))
                            .collect();
                        order.sort_by(|a, b| a.0.total_cmp(&b.0));
                        for &(_, m, inst) in &order {
                            for pass in 0..3 {
                                match pass {
                                    0 => rp.set_pipeline(&self.pipe_tr_depth),
                                    1 => rp.set_pipeline(&self.pipe_tr),
                                    _ => {
                                        if !vs.edges {
                                            continue;
                                        }
                                        rp.set_pipeline(&self.pipe_edge)
                                    }
                                }
                                for p in &gs.pieces[m] {
                                    let ch = &gs.chunks[p.chunk as usize];
                                    let (buf, start, count) = if pass < 2 { (&ch.tri_ibuf, p.tri_start, p.tri_count) } else { (&ch.edge_ibuf, p.edge_start, p.edge_count) };
                                    let Some(b) = buf else { continue };
                                    if count == 0 {
                                        continue;
                                    }
                                    rp.set_vertex_buffer(0, ch.vbuf.slice(..));
                                    rp.set_index_buffer(b.slice(..), wgpu::IndexFormat::Uint32);
                                    rp.draw_indexed(start..start + count, p.base_vertex, inst..inst + 1);
                                }
                            }
                        }
                    }
                }
                if let Some((vb, ib, n)) = &gs.hl {
                    rp.set_pipeline(&self.pipe_hl);
                    rp.set_vertex_buffer(0, vb.slice(..));
                    rp.set_vertex_buffer(1, ib.slice(..));
                    rp.draw(0..*n, 0..1);
                }
            }
            if let Some(lb) = &line_buf {
                rp.set_pipeline(&self.pipe_line);
                rp.set_vertex_buffer(0, lb.slice(..));
                rp.draw(0..vs.lines.len() as u32, 0..1);
            }
        }
        // sRGB → same bytes in a plain RGBA texture (copies may differ only in sRGB-ness)
        enc.copy_texture_to_texture(t.resolve.as_image_copy(), t.display.as_image_copy(), wgpu::Extent3d { width: t.size[0], height: t.size[1], depth_or_array_layers: 1 });
        queue.submit([enc.finish()]);
    }
}

/// Pixel offsets of the edge passes for a line `width` pixels wide (1 = a single pass).
fn edge_offsets(width: f32) -> Vec<[f32; 2]> {
    let r = ((width - 1.0) * 0.5).clamp(0.0, 1.5);
    if r < 0.05 {
        return vec![[0.0, 0.0]];
    }
    let mut v = vec![[0.0, 0.0]];
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        v.push([a.cos() * r, a.sin() * r]);
    }
    v.truncate(PASS_SLOTS);
    v
}
