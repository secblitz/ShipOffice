//! The office's wgpu pass, plugged into iced through the shader widget.

use super::assets::{self, decode, meta, mips, premultiply, split_sheet, MAX_SHADE};
use super::city::Sky;
use super::{DrawList, Fx, Instance, TexKey};
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::Rectangle;
use image::RgbaImage;
use std::collections::HashMap;

#[derive(Debug)]
pub struct Primitive {
    pub list: DrawList,
    /// world px -> stage px: screen = world · scale + origin
    pub scale: f32,
    pub origin: (f32, f32),
}

struct Tex {
    bind: wgpu::BindGroup,
    version: u64,
    bytes: u64,
    used: u64,
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    zero: wgpu::TextureView,
    textures: HashMap<TexKey, Tex>,
    instances: wgpu::Buffer,
    capacity: u64,
    runs: Vec<(TexKey, u32, u32)>,
    srgb: bool,
    frame: u64,
    city: City,
}

/// The sky pass: one triangle over the stage, ray casting the city (sky.wgsl).
struct City {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniforms_bind: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// day and night textures, made the first time each is shown
    tex: [Option<wgpu::BindGroup>; 2],
    /// this frame's camera, if the city is on
    now: Option<(Sky, usize)>,
}

impl City {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("city"), source: wgpu::ShaderSource::Wgsl(include_str!("sky.wgsl").into()) });
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("city uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("city textures"),
            entries: &[tex_entry(0), tex_entry(1), tex_entry(2), wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("city"), bind_group_layouts: &[&uniforms_layout, &tex_layout], push_constant_ranges: &[] });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("city"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("city uniforms"), size: std::mem::size_of::<Sky>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let uniforms_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("city uniforms"), layout: &uniforms_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }] });
        // the panoramas wrap around the horizon and the lots tile; anisotropy keeps the street
        // sharp when it is seen at a slant
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("city"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        Self { pipeline, uniforms, uniforms_bind, tex_layout, sampler, tex: [None, None], now: None }
    }

    fn textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, night: usize) -> Option<u64> {
        if self.tex[night].is_some() {
            return Some(0);
        }
        let time = ["day", "night"][night];
        let mut bytes = 0;
        let mut views = Vec::new();
        for (layer, levels) in [("far", 6), ("near", 6), ("ground", 9)] {
            let mut img = decode(&format!("city/{layer}-{time}.webp"))?;
            premultiply(&mut img);
            let (v, b) = upload(device, queue, &img, levels);
            bytes += b;
            views.push(v);
        }
        self.tex[night] = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("city textures"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[0]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[1]) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[2]) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }));
        Some(bytes)
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    scale: [f32; 2],
    offset: [f32; 2],
    srgb_target: f32,
    max_shade: f32,
    _pad: [f32; 2],
}

const MIP_LEVELS: u32 = 4;
/// textures not drawn for this many frames are dropped (another tab's crew, say), but only once
/// the office textures hold more than `BUDGET`: decoding a sheet again costs far more than keeping it
const EVICT_AFTER: u64 = 600;
const BUDGET: u64 = 256 * 1024 * 1024;

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("office"), source: wgpu::ShaderSource::Wgsl(include_str!("office.wgsl").into()) });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("office globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("office textures"),
            entries: &[tex_entry(0), tex_entry(1), wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("office"), bind_group_layouts: &[&globals_layout, &tex_layout], push_constant_ranges: &[] });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4,
            4 => Unorm8x4, 5 => Unorm8x4, 6 => Unorm8x4, 7 => Unorm8x4, 8 => Unorm8x4
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("office"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Instance>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &attrs }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor { label: Some("office globals"), size: std::mem::size_of::<Globals>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("office globals"), layout: &globals_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }] });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("office"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let zero = device.create_texture(&tex_desc(1, 1, 1)).create_view(&Default::default());
        let capacity = 1024;
        let instances = device.create_buffer(&wgpu::BufferDescriptor { label: Some("office instances"), size: capacity * std::mem::size_of::<Instance>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let city = City::new(device, format);
        Self { pipeline, globals, globals_bind, tex_layout, sampler, zero, textures: HashMap::new(), instances, capacity, runs: Vec::new(), srgb: format.is_srgb(), frame: 0, city }
    }

    fn trim(&mut self) {
        if self.texture_bytes() <= BUDGET {
            return;
        }
        let frame = self.frame;
        self.textures.retain(|k, t| matches!(k, TexKey::Fx(_) | TexKey::Atlas(_)) || frame - t.used < EVICT_AFTER);
    }
}

fn tex_desc(w: u32, h: u32, levels: u32) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("office texture"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    }
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, img: &RgbaImage, levels: u32) -> (wgpu::TextureView, u64) {
    let chain = mips(img, levels);
    let tex = device.create_texture(&tex_desc(img.width().max(1), img.height().max(1), chain.len() as u32));
    let mut bytes = 0;
    for (lvl, m) in chain.iter().enumerate() {
        bytes += m.as_raw().len() as u64;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: lvl as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            m.as_raw(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * m.width()), rows_per_image: Some(m.height()) },
            wgpu::Extent3d { width: m.width(), height: m.height(), depth_or_array_layers: 1 },
        );
    }
    (tex.create_view(&Default::default()), bytes)
}

/// Small procedural textures: soft shadow, selection ring, a dot, a glass pane.
fn fx_image(fx: Fx) -> RgbaImage {
    let (w, h) = match fx {
        Fx::White => (4, 4),
        Fx::Shadow | Fx::Ring => (128, 56),
        Fx::Dot => (32, 32),
        Fx::Glass => (16, 64),
        Fx::Ramp => (4, 64),
    };
    RgbaImage::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0, (y as f32 + 0.5) / h as f32 * 2.0 - 1.0);
        let r = (u * u + v * v).sqrt();
        let a = match fx {
            Fx::White => 1.0,
            Fx::Shadow => (1.0 - r).clamp(0.0, 1.0).powf(0.8),
            // a 2px world-space ring on a 36x16 quad
            Fx::Ring => {
                let d = (r - 0.86).abs();
                (1.0 - d / 0.07).clamp(0.0, 1.0)
            }
            Fx::Dot => ((1.0 - r) * 8.0).clamp(0.0, 1.0),
            Fx::Ramp => {
                let t = (y as f32 + 0.5) / h as f32;
                t * t * (3.0 - 2.0 * t)
            }
            Fx::Glass => {
                let edge = x == 0 || x == w - 1 || y == 0 || y == h - 1;
                if edge {
                    0.55
                } else {
                    0.22
                }
            }
        };
        let (cr, cg, cb) = match fx {
            Fx::Glass if x == 0 || x == w - 1 || y == 0 || y == h - 1 => (220.0, 235.0, 245.0),
            Fx::Glass => (170.0, 205.0, 230.0),
            _ => (255.0, 255.0, 255.0),
        };
        image::Rgba([(cr * a) as u8, (cg * a) as u8, (cb * a) as u8, (255.0 * a) as u8])
    })
}

impl Pipeline {
    fn bind(&self, device: &wgpu::Device, color: &wgpu::TextureView, weights: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("office texture"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(color) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(weights) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, key: &TexKey, layers: &[super::Layer]) {
        let layer = match key {
            TexKey::Layer(id) => layers.iter().find(|l| l.id == *id),
            _ => None,
        };
        if let Some(t) = self.textures.get_mut(key) {
            if layer.is_none_or(|l| l.version == t.version) {
                t.used = self.frame;
                return;
            }
        }
        let made = match key {
            TexKey::Sheet(name) => meta().anims.get(*name).and_then(|a| {
                let src = decode(&a.file)?;
                let mask = a.mask.as_deref().and_then(assets::file).and_then(|b| image::load_from_memory(b).ok()).map(|m| m.to_luma8());
                Some(match mask {
                    Some(mask) if mask.dimensions() == src.dimensions() => {
                        let (fixed, weights) = split_sheet(&src, &mask);
                        let (c, b1) = upload(device, queue, &fixed, MIP_LEVELS);
                        let (w, b2) = upload(device, queue, &weights, MIP_LEVELS);
                        (self.bind(device, &c, &w), 0, b1 + b2)
                    }
                    _ => {
                        let mut src = src;
                        premultiply(&mut src);
                        let (c, b) = upload(device, queue, &src, MIP_LEVELS);
                        (self.bind(device, &c, &self.zero), 0, b)
                    }
                })
            }),
            TexKey::Atlas(file) => decode(file).map(|mut img| {
                premultiply(&mut img);
                let (c, b) = upload(device, queue, &img, MIP_LEVELS);
                (self.bind(device, &c, &self.zero), 0, b)
            }),
            TexKey::Fx(fx) => {
                let (c, b) = upload(device, queue, &fx_image(*fx), 1);
                Some((self.bind(device, &c, &self.zero), 0, b))
            }
            TexKey::Layer(_) => layer.map(|l| {
                let (c, b) = upload(device, queue, &l.image, 3);
                (self.bind(device, &c, &self.zero), l.version, b)
            }),
        };
        if let Some((bind, version, bytes)) = made {
            self.textures.insert(key.clone(), Tex { bind, version, bytes, used: self.frame });
        }
    }

    /// GPU memory held by office textures, for the perf readout.
    pub fn texture_bytes(&self) -> u64 {
        self.textures.values().map(|t| t.bytes).sum()
    }
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(&self, p: &mut Pipeline, device: &wgpu::Device, queue: &wgpu::Queue, bounds: &Rectangle, _viewport: &Viewport) {
        p.frame += 1;
        let (w, h) = (bounds.width.max(1.0), bounds.height.max(1.0));
        let g = Globals {
            scale: [2.0 * self.scale / w, -2.0 * self.scale / h],
            offset: [2.0 * self.origin.0 / w - 1.0, 1.0 - 2.0 * self.origin.1 / h],
            srgb_target: if p.srgb { 1.0 } else { 0.0 },
            max_shade: MAX_SHADE,
            _pad: [0.0; 2],
        };
        queue.write_buffer(&p.globals, 0, bytemuck::bytes_of(&g));
        p.city.now = None;
        if let Some(mut sky) = self.list.sky {
            sky.lens[3] = if p.srgb { 1.0 } else { 0.0 };
            let night = (sky.lens[2] > 0.5) as usize;
            if p.city.textures(device, queue, night).is_some() {
                queue.write_buffer(&p.city.uniforms, 0, bytemuck::bytes_of(&sky));
                p.city.now = Some((sky, night));
            }
        }
        p.runs.clear();
        let items = &self.list.items;
        if items.len() as u64 > p.capacity {
            p.capacity = (items.len() as u64).next_power_of_two();
            p.instances = device.create_buffer(&wgpu::BufferDescriptor { label: Some("office instances"), size: p.capacity * std::mem::size_of::<Instance>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        }
        let data: Vec<Instance> = items.iter().map(|(_, i)| *i).collect();
        if !data.is_empty() {
            queue.write_buffer(&p.instances, 0, bytemuck::cast_slice(&data));
        }
        for (i, (key, _)) in items.iter().enumerate() {
            match p.runs.last_mut() {
                Some((k, _, end)) if k == key => *end = i as u32 + 1,
                _ => {
                    p.ensure(device, queue, key, &self.list.layers);
                    p.runs.push((key.clone(), i as u32, i as u32 + 1));
                }
            }
        }
    }

    fn draw(&self, p: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if let Some((_, night)) = p.city.now {
            if let Some(tex) = &p.city.tex[night] {
                pass.set_pipeline(&p.city.pipeline);
                pass.set_bind_group(0, &p.city.uniforms_bind, &[]);
                pass.set_bind_group(1, tex, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        if p.runs.is_empty() {
            return true;
        }
        pass.set_pipeline(&p.pipeline);
        pass.set_bind_group(0, &p.globals_bind, &[]);
        pass.set_vertex_buffer(0, p.instances.slice(..));
        for (key, start, end) in &p.runs {
            if let Some(t) = p.textures.get(key) {
                pass.set_bind_group(1, &t.bind, &[]);
                pass.draw(0..6, *start..*end);
            }
        }
        true
    }
}
