//! The wgpu layer: device acquisition, two window surfaces, one shared WGSL
//! dread shader, and the two renderers (main 3D scene / monster overlay).
//!
//! The single shader handles every draw in the game:
//!   flags.x < 0.5  -> procedural path (corridor floor grid, tablet bezel)
//!   flags.x >= 0.5 -> textured path (mirror frame or sprite frame) with
//!                     chromatic aberration, row-glitching, CRT scan + flicker.

use std::sync::Arc;

use winit::window::Window;

use crate::math::{Mat4, Vec3};
use crate::mirror::{MirrorFrame, MIRROR_H, MIRROR_W};
use crate::sprite::{GOODBYE_H, GOODBYE_W, SHEET_COLS, SHEET_ROWS, FRAME_PX};

const SHADER: &str = r#"
struct Params {
    mvp      : mat4x4<f32>,
    uv_scale : vec2<f32>,
    uv_offset: vec2<f32>,
    tint     : vec4<f32>,
    flags    : vec4<f32>, // x: texture_mode  y: time  z: glitch  w: crt
};

@group(0) @binding(0) var<uniform> params : Params;
@group(0) @binding(1) var tex : texture_2d<f32>;
@group(0) @binding(2) var samp : sampler;

struct VSOut {
    @builtin(position) pos : vec4<f32>,
    @location(0) uv : vec2<f32>,
};

@vertex
fn vs_main(@location(0) in_pos : vec3<f32>, @location(1) in_uv : vec2<f32>) -> VSOut {
    var out : VSOut;
    out.pos = params.mvp * vec4<f32>(in_pos, 1.0);
    out.uv = in_uv;
    return out;
}

@fragment
fn fs_main(vin : VSOut) -> @location(0) vec4<f32> {
    if (params.flags.x < 0.5) {
        // procedural path: the infinite corridor floor
        let g = vec2<f32>(fract(vin.uv.x * 9.0), fract(vin.uv.y * 9.0));
        let line = max(select(0.0, 1.0, g.x < 0.035), select(0.0, 1.0, g.y < 0.035));
        let fade = clamp(1.30 - length(vin.uv - vec2<f32>(0.5, 0.5)) * 1.9, 0.0, 1.0);
        let col = mix(vec3<f32>(0.015, 0.012, 0.018), vec3<f32>(0.38, 0.05, 0.07), line);
        return vec4<f32>(col * (0.22 + 0.78 * fade), params.tint.a);
    }

    var tuv = vin.uv * params.uv_scale + params.uv_offset;
    let t = params.flags.y;
    let glitch = params.flags.z;

    if (glitch > 0.001) {
        // horizontal tearing: per-row hash displacement, reseeded over time
        let row = floor(vin.uv.y * 90.0);
        let h = fract(sin(row * 91.17 + floor(t * 24.0) * 13.71) * 43758.547);
        tuv.x = tuv.x + (h - 0.5) * 0.09 * glitch;
    }

    let center = textureSample(tex, samp, tuv);
    let aberr = 0.0012 + 0.0065 * glitch;
    let r = textureSample(tex, samp, tuv + vec2<f32>(aberr, 0.0)).r;
    let b = textureSample(tex, samp, tuv - vec2<f32>(aberr, 0.0)).b;
    var col = vec4<f32>(r, center.g, b, center.a);

    if (params.flags.w > 0.5) {
        let scan = 0.90 + 0.10 * sin(vin.uv.y * 620.0 + t * 9.0);
        let flick = 0.93 + 0.07 * fract(sin(dot(vec2<f32>(floor(t * 47.0), 3.7), vec2<f32>(12.9898, 78.233))) * 43758.5453);
        col = vec4<f32>(col.rgb * scan * flick, col.a);
    }

    return vec4<f32>(col.rgb * params.tint.rgb, col.a * params.tint.a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: 20,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

/// Uniform payload — layout mirrors the WGSL `Params` struct byte-for-byte (112 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuParams {
    pub mvp: [[f32; 4]; 4],
    pub uv_scale: [f32; 2],
    pub uv_offset: [f32; 2],
    pub tint: [f32; 4],
    pub flags: [f32; 4],
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// Instance + adapter (bound against the main surface so presentation is
    /// guaranteed) + device/queue. Returns the main window's surface for reuse.
    pub fn for_window(window: &Arc<Window>) -> (Self, wgpu::Surface<'static>) {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .expect("failed to bind the main window to a GPU surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no suitable GPU adapter was found");
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("phantasm-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            },
            None,
        ))
        .expect("failed to acquire the GPU device queue");
        (
            Gpu { instance, adapter, device, queue },
            surface,
        )
    }
}

// ----------------------------------------------------------------------
// Shared pipeline machinery
// ----------------------------------------------------------------------

struct Pipelines {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

fn build_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> Pipelines {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("phantasm-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("phantasm-pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("phantasm-shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("phantasm-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            compilation_options: Default::default(),
            buffers: &[Vertex::desc()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    Pipelines { pipeline, bgl }
}

fn make_bind_group(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("phantasm-bg"),
        layout: bgl,
        entries: &[
            wgpu::Bind
