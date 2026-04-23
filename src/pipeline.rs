//! Single graphics pipeline used to draw every 2D primitive in the game.
//!
//! Pipeline summary:
//!
//! * Vertex input: per vertex `vec2 position` and `vec4 color`,
//!   packed into a single interleaved binding. The alpha channel
//!   was added so translucent overlays (darkening layer behind the
//!   slide-out music panel) can blend with the geometry beneath
//!   them without duplicating the pipeline or the vertex format.
//! * Topology:    triangle list, no index buffer, no instancing.
//! * Viewport / scissor: marked dynamic, set per frame from the
//!   renderer so a window resize never requires pipeline recreation.
//! * Rasterizer:  fill, no culling.
//! * Multisampling: disabled (1 sample).
//! * Color blend: straight alpha blending.
//! * Depth / stencil: not attached.
//!
//! Push constants layout (16 bytes):
//!
//! * `vec2 scale`  — aspect correction scale factors.
//! * `vec2 shake`  — per-frame screen-shake offset in game space.
//!
//! The shake is applied inside the vertex shader BEFORE the aspect
//! scale so it reads as a true translation of the playfield rather
//! than a stretched displacement that depends on window shape.

use ash::{vk, Device};
use std::ffi::CStr;

const VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/main.vert.spv"));
const FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/main.frag.spv"));

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub pos:   [f32; 2],
    pub color: [f32; 4],
}

impl Vertex {
    #[inline]
    pub fn opaque(pos: [f32; 2], rgb: [f32; 3]) -> Self {
        Vertex { pos, color: [rgb[0], rgb[1], rgb[2], 1.0] }
    }
    #[inline]
    pub fn rgba(pos: [f32; 2], rgb: [f32; 3], a: f32) -> Self {
        Vertex { pos, color: [rgb[0], rgb[1], rgb[2], a] }
    }
}

/// Matches the `PushConstants` block in main.vert byte for byte.
/// Layout in v2:
///
///   0..8   vec2 scale  (aspect correction)
///   8..16  vec2 shake  (screen shake offset in game space)
///   16..20 float zoom  (camera zoom; 1.0 is neutral)
///   20..32 float _pad[3]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PushConstants {
    pub scale: [f32; 2],
    pub shake: [f32; 2],
    pub zoom:  f32,
    pub _pad:  [f32; 3],
}

pub struct Pipeline {
    pub layout:   vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
}

impl Pipeline {
    pub fn new(device: &Device, render_pass: vk::RenderPass) -> Self {
        unsafe {
            let vert = create_shader_module(device, VERT_SPV);
            let frag = create_shader_module(device, FRAG_SPV);

            let entry = CStr::from_bytes_with_nul(b"main\0").unwrap();

            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(vert)
                    .name(entry),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(frag)
                    .name(entry),
            ];

            let bindings = [vk::VertexInputBindingDescription {
                binding:    0,
                stride:     std::mem::size_of::<Vertex>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            }];
            let attrs = [
                vk::VertexInputAttributeDescription {
                    location: 0, binding: 0,
                    format:   vk::Format::R32G32_SFLOAT, offset: 0,
                },
                vk::VertexInputAttributeDescription {
                    location: 1, binding: 0,
                    format:   vk::Format::R32G32B32A32_SFLOAT, offset: 8,
                },
            ];
            let vi = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attrs);

            let ia = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1).scissor_count(1);

            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);

            let ms = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);

            let blend_attach = [vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                .alpha_blend_op(vk::BlendOp::ADD)];
            let blend = vk::PipelineColorBlendStateCreateInfo::default()
                .attachments(&blend_attach);

            let dyn_states = [
                vk::DynamicState::VIEWPORT,
                vk::DynamicState::SCISSOR,
            ];
            let dynamic = vk::PipelineDynamicStateCreateInfo::default()
                .dynamic_states(&dyn_states);

            // Push constants now hold (scale, shake), 16 bytes total.
            let push_ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(std::mem::size_of::<PushConstants>() as u32)];
            let layout_info = vk::PipelineLayoutCreateInfo::default()
                .push_constant_ranges(&push_ranges);
            let layout = device.create_pipeline_layout(&layout_info, None).unwrap();

            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vi)
                .input_assembly_state(&ia)
                .viewport_state(&viewport_state)
                .rasterization_state(&raster)
                .multisample_state(&ms)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(layout)
                .render_pass(render_pass)
                .subpass(0);

            let pipeline = device.create_graphics_pipelines(
                vk::PipelineCache::null(), &[info], None,
            ).expect("vkCreateGraphicsPipelines failed")[0];

            device.destroy_shader_module(vert, None);
            device.destroy_shader_module(frag, None);

            Pipeline { layout, pipeline }
        }
    }

    pub fn destroy(&self, device: &Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

unsafe fn create_shader_module(device: &Device, code: &[u8]) -> vk::ShaderModule {
    assert!(code.len() % 4 == 0, "SPIR-V blob length must be a multiple of 4");
    let words: Vec<u32> = code.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    device.create_shader_module(&info, None).unwrap()
}