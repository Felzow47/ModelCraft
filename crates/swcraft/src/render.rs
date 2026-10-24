//! wgpu renderer: draws the scene into an offscreen MSAA texture that egui displays.

use bytemuck::{Pod, Zeroable};
use eframe::wgpu;
use glam::Mat4;
use sw_format::Scene;
use wgpu::util::DeviceExt;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    nrm: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    model: [f32; 16],
    tint: [f32; 4],
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
}

#[derive(Clone, Copy, Default)]
struct MeshRange {
    base_vertex: i32,
    tri_start: u32,
    tri_count: u32,
    edge_start: u32,
    edge_count: u32,
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
}

pub struct ViewSettings {
    pub view_proj: Mat4,
    pub view: Mat4,
    pub ortho: bool,
    pub edges: bool,
    pub base_color: [f32; 3],
}

pub struct Renderer {
    pipe_mesh: wgpu::RenderPipeline,
    pipe_edge: wgpu::RenderPipeline,
    pipe_bg: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind: wgpu::BindGroup,
    target: Option<Target>,
    pub scene: Option<GpuScene>,
}

pub struct Target {
    pub size: [u32; 2],
    msaa: wgpu::TextureView,
    #[allow(dead_code)]
    pub resolve: wgpu::Texture,
    pub resolve_view: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("swcraft shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let iattrs = wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
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
        let ms = wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false };
        let target = [Some(wgpu::ColorTargetState {
            format: COLOR_FORMAT,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipe_mesh = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 },
            }),
            multisample: ms,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let pipe_edge = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("edges"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_edge"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: ms,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_edge"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            multiview_mask: None,
            cache: None,
        });
        let pipe_bg = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("background"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_bg"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: ms,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_bg"),
                compilation_options: Default::default(),
                targets: &target,
            }),
            multiview_mask: None,
            cache: None,
        });
        Renderer { pipe_mesh, pipe_edge, pipe_bg, globals, bind, target: None, scene: None }
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
        let msaa = mk("msaa", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = mk(
            "resolve",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = mk("depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        self.target = Some(Target {
            size,
            msaa: msaa.create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
            resolve,
            depth: depth.create_view(&Default::default()),
        });
        true
    }

    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    /// Upload all meshes of a scene.
    pub fn upload_scene(&mut self, device: &wgpu::Device, scene: &Scene) {
        let mut verts: Vec<Vertex> = Vec::new();
        let mut tris: Vec<u32> = Vec::new();
        let mut edges: Vec<u32> = Vec::new();
        let mut ranges = Vec::with_capacity(scene.meshes.len());
        for m in &scene.meshes {
            let m = &m.mesh;
            let r = MeshRange {
                base_vertex: verts.len() as i32,
                tri_start: tris.len() as u32,
                tri_count: m.indices.len() as u32,
                edge_start: edges.len() as u32,
                edge_count: m.edges.len() as u32,
            };
            verts.extend(m.positions.iter().zip(&m.normals).map(|(p, n)| Vertex { pos: *p, nrm: *n }));
            tris.extend_from_slice(&m.indices);
            edges.extend_from_slice(&m.edges);
            ranges.push(r);
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
        self.scene = Some(GpuScene { vbuf, tri_ibuf, edge_ibuf, ranges, inst_buf: None, batches: Vec::new() });
    }

    /// Rebuild the per-instance buffer. `visible[i]` and `tints[i]` are per component.
    pub fn update_instances(&mut self, device: &wgpu::Device, scene: &Scene, visible: &[bool], tints: &[[f32; 4]]) {
        let Some(gs) = self.scene.as_mut() else { return };
        let mut items: Vec<(usize, Instance)> = scene
            .components
            .iter()
            .enumerate()
            .filter(|(i, _)| visible.get(*i).copied().unwrap_or(true))
            .filter_map(|(i, c)| {
                c.mesh.map(|m| (m, Instance { model: c.world, tint: tints.get(i).copied().unwrap_or([0.0; 4]) }))
            })
            .collect();
        items.sort_by_key(|(m, _)| *m);
        let mut batches: Vec<(usize, u32, u32)> = Vec::new();
        for (k, (m, _)) in items.iter().enumerate() {
            match batches.last_mut() {
                Some(b) if b.0 == *m => b.2 += 1,
                _ => batches.push((*m, k as u32, 1)),
            }
        }
        let inst: Vec<Instance> = items.into_iter().map(|(_, i)| i).collect();
        gs.inst_buf = (!inst.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instances"),
                contents: bytemuck::cast_slice(&inst),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        gs.batches = batches;
    }

    pub fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, vs: &ViewSettings) {
        let Some(t) = &self.target else { return };
        let g = Globals {
            view_proj: vs.view_proj.to_cols_array(),
            view: vs.view.to_cols_array(),
            base_color: [vs.base_color[0], vs.base_color[1], vs.base_color[2], 1.0],
            edge_color: [0.08, 0.09, 0.11, 0.85],
            bg_top: [0.36, 0.40, 0.46, 1.0],
            bg_bottom: [0.12, 0.13, 0.16, 1.0],
            params: [if vs.ortho { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    depth_slice: None,
                    resolve_target: Some(&t.resolve_view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Discard },
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
            rp.set_bind_group(0, &self.bind, &[]);
            rp.set_pipeline(&self.pipe_bg);
            rp.draw(0..3, 0..1);
            if let Some(gs) = &self.scene {
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
                            for &(m, first, count) in &gs.batches {
                                let r = gs.ranges[m];
                                if r.edge_count > 0 {
                                    rp.draw_indexed(r.edge_start..r.edge_start + r.edge_count, r.base_vertex, first..first + count);
                                }
                            }
                        }
                    }
                }
            }
        }
        queue.submit([enc.finish()]);
    }
}
