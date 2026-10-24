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
}

#[derive(Clone, Copy, Default)]
struct MeshRange {
    base_vertex: i32,
    tri_start: u32,
    tri_count: u32,
    edge_start: u32,
    edge_count: u32,
}

/// One drawable instance of a mesh.
pub struct InstanceDesc {
    pub mesh: usize,
    pub model: [f32; 16],
    /// Selection / hover tint (alpha = strength).
    pub tint: [f32; 4],
    /// Own colour from the file (alpha 0 = use the base colour).
    pub color: [f32; 4],
}

/// GPU copy of a loaded scene.
pub struct GpuScene {
    vbuf: wgpu::Buffer,
    tri_ibuf: wgpu::Buffer,
    edge_ibuf: Option<wgpu::Buffer>,
    ranges: Vec<MeshRange>,
    inst_buf: Option<wgpu::Buffer>,
    /// (mesh index, first instance, instance count)
    batches: Vec<(usize, u32, u32)>,
    /// Highlighted triangles (already in world space): vertices, instance, index count.
    hl: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
}

impl GpuScene {
    pub fn upload(device: &wgpu::Device, scene: &Scene) -> GpuScene {
        let mut verts: Vec<Vertex> = Vec::new();
        let mut tris: Vec<u32> = Vec::new();
        let mut edges: Vec<u32> = Vec::new();
        let mut ranges = Vec::with_capacity(scene.meshes.len());
        for m in &scene.meshes {
            let m = &m.mesh;
            ranges.push(MeshRange {
                base_vertex: verts.len() as i32,
                tri_start: tris.len() as u32,
                tri_count: m.indices.len() as u32,
                edge_start: edges.len() as u32,
                edge_count: m.edges.len() as u32,
            });
            verts.extend(m.positions.iter().zip(&m.normals).map(|(p, n)| Vertex { pos: *p, nrm: *n }));
            tris.extend_from_slice(&m.indices);
            edges.extend_from_slice(&m.edges);
        }
        if verts.is_empty() {
            verts.push(Vertex::zeroed());
        }
        if tris.is_empty() {
            tris.extend_from_slice(&[0, 0, 0]);
        }
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertices"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let tri_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("triangles"),
            contents: bytemuck::cast_slice(&tris),
            usage: wgpu::BufferUsages::INDEX,
        });
        let edge_ibuf = (!edges.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("edges"),
                contents: bytemuck::cast_slice(&edges),
                usage: wgpu::BufferUsages::INDEX,
            })
        });
        GpuScene { vbuf, tri_ibuf, edge_ibuf, ranges, inst_buf: None, batches: Vec::new(), hl: None }
    }

    pub fn set_instances(&mut self, device: &wgpu::Device, mut items: Vec<InstanceDesc>) {
        items.sort_by_key(|i| i.mesh);
        let mut batches: Vec<(usize, u32, u32)> = Vec::new();
        for (k, it) in items.iter().enumerate() {
            match batches.last_mut() {
                Some(b) if b.0 == it.mesh => b.2 += 1,
                _ => batches.push((it.mesh, k as u32, 1)),
            }
        }
        let inst: Vec<Instance> = items.into_iter().map(|i| Instance { model: i.model, tint: i.tint, color: i.color }).collect();
        self.inst_buf = (!inst.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instances"),
                contents: bytemuck::cast_slice(&inst),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        self.batches = batches;
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
        let inst = Instance { model: Mat4::IDENTITY.to_cols_array(), tint: color, color: [0.0; 4] };
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
}

impl Light {
    pub fn preset(p: crate::settings::Lighting) -> Light {
        use crate::settings::Lighting::*;
        match p {
            Studio => Light { key: 0.85, fill: 0.30, ambient: 0.23, spec: 0.35, rim: 0.12, flat: false, hemi: 0.3, shininess: 48.0 },
            Soft => Light { key: 0.55, fill: 0.35, ambient: 0.42, spec: 0.12, rim: 0.06, flat: false, hemi: 0.15, shininess: 24.0 },
            Contrast => Light { key: 1.05, fill: 0.15, ambient: 0.12, spec: 0.55, rim: 0.2, flat: false, hemi: 0.45, shininess: 80.0 },
            Flat => Light { key: 0.0, fill: 0.0, ambient: 1.0, spec: 0.0, rim: 0.0, flat: true, hemi: 0.0, shininess: 1.0 },
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
    /// Grid / bounding box lines (line list, world space).
    pub lines: Vec<LineVertex>,
}

pub struct Renderer {
    pub samples: u32,
    pipe_mesh: wgpu::RenderPipeline,
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
        let iattrs = wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4];
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
                    ds: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers: bufs },
                primitive: wgpu::PrimitiveState { topology: topo, cull_mode: None, ..Default::default() },
                depth_stencil: ds,
                multisample: ms,
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &target }),
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
        );
        let pipe_hl = pipe(
            "highlight",
            "vs_mesh",
            "fs_mesh",
            &buffers,
            T::TriangleList,
            depth(false, C::LessEqual, wgpu::DepthBiasState { constant: -1, slope_scale: -1.0, clamp: 0.0 }),
        );
        let pipe_edge = pipe("edges", "vs_edge", "fs_edge", &buffers, T::LineList, depth(false, C::LessEqual, Default::default()));
        let pipe_line = pipe("lines", "vs_line", "fs_line", &line_buffers, T::LineList, depth(false, C::LessEqual, Default::default()));
        let pipe_bg = pipe("background", "vs_bg", "fs_bg", &[], T::TriangleList, depth(false, C::Always, Default::default()));
        Renderer { samples, pipe_mesh, pipe_hl, pipe_edge, pipe_line, pipe_bg, globals, passes, bind, target: None }
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
        self.target = Some(Target {
            size,
            msaa,
            resolve_view: resolve.create_view(&Default::default()),
            resolve,
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
                    rp.set_vertex_buffer(0, gs.vbuf.slice(..));
                    rp.set_vertex_buffer(1, ib.slice(..));
                    rp.set_pipeline(&self.pipe_mesh);
                    rp.set_index_buffer(gs.tri_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    for &(m, first, count) in &gs.batches {
                        let r = gs.ranges[m];
                        if r.tri_count > 0 {
                            rp.draw_indexed(r.tri_start..r.tri_start + r.tri_count, r.base_vertex, first..first + count);
                        }
                    }
                    if vs.edges {
                        if let Some(eb) = &gs.edge_ibuf {
                            rp.set_pipeline(&self.pipe_edge);
                            rp.set_index_buffer(eb.slice(..), wgpu::IndexFormat::Uint32);
                            for k in 0..offsets.len() {
                                rp.set_bind_group(0, &self.bind, &[(k as u64 * PASS_STRIDE) as u32]);
                                for &(m, first, count) in &gs.batches {
                                    let r = gs.ranges[m];
                                    if r.edge_count > 0 {
                                        rp.draw_indexed(r.edge_start..r.edge_start + r.edge_count, r.base_vertex, first..first + count);
                                    }
                                }
                            }
                            rp.set_bind_group(0, &self.bind, &[0]);
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
