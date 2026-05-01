//! Safe Vulkan pipeline construction for validated user SPIR-V.
//!
//! Given a blob that has already passed `spirv::validate`,
//! build a graphics pipeline compatible with the engine's
//! post pass: one fullscreen triangle, one combined image
//! sampler at set 0 binding 0, push constants at fragment
//! stage. The result wraps the pipeline and shader module so
//! the renderer can destroy them together when switching
//! back to the default.
//!
//! This function never enables additional features, never
//! requests extra sets, and never binds additional
//! resources. Everything the user shader sees is what the
//! default post pass already provides, which means the
//! renderer can swap between the two at any subpass boundary
//! without recreating descriptor sets.

use std::ffi::CStr;

use ash::{vk, Device};

/// Result of a successful pipeline build. Owns its pipeline
/// and shader module. The caller destroys both through
/// `UserPipeline::destroy` when done.
pub struct UserPipeline {
    pub id:       String,
    pub pipeline: vk::Pipeline,
    pub module:   vk::ShaderModule,
    pub layout:   vk::PipelineLayout,
    /// True while the renderer owns the `layout`. User
    /// pipelines build their own layout so they can declare
    /// a push constant range sized for the user's parameter
    /// block; when the pipeline is destroyed the layout
    /// goes with it.
    pub owns_layout: bool,
}

impl UserPipeline {
    pub fn destroy(&self, device: &Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_shader_module(self.module, None);
            if self.owns_layout {
                device.destroy_pipeline_layout(self.layout, None);
            }
        }
    }
}

/// Build a post pipeline from `spirv`. `render_pass` and
/// `set_layout` come from the engine's `PostStage` so the
/// user shader samples the same offscreen target.
pub fn build(
    device:      &Device,
    render_pass: vk::RenderPass,
    set_layout:  vk::DescriptorSetLayout,
    spirv:       &[u8],
    id:          String,
) -> Result<UserPipeline, vk::Result> {
    unsafe {
        let words = slice_to_u32(spirv);
        let module_info = vk::ShaderModuleCreateInfo::default()
            .code(&words);
        let frag_module = device.create_shader_module(&module_info, None)?;

        // Vertex stage reuses the engine's fullscreen vertex
        // shader so we do not ship two copies of the same
        // three line program. The module is inlined here via
        // include_bytes to keep the file dependency local.
        let vert_bytes: &[u8] = include_bytes!(
            concat!(env!("OUT_DIR"), "/post.vert.spv"));
        let vert_words = slice_to_u32(vert_bytes);
        let vert_info = vk::ShaderModuleCreateInfo::default()
            .code(&vert_words);
        let vert_module = device.create_shader_module(&vert_info, None)?;

        let entry = CStr::from_bytes_with_nul(b"main\0").unwrap();
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module).name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module).name(entry),
        ];

        let set_layouts = [set_layout];
        // Generous 128 byte push constant range so user
        // shaders have room for parameters. The runtime
        // writer is responsible for never overflowing this.
        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(128)];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_ranges);
        let layout = device.create_pipeline_layout(&layout_info, None)?;

        let vi = vk::PipelineVertexInputStateCreateInfo::default();
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
            .blend_enable(false)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&blend_attach);
        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default()
            .dynamic_states(&dyn_states);

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

        let pipelines = device.create_graphics_pipelines(
            vk::PipelineCache::null(), &[info], None);
        let pipeline = match pipelines {
            Ok(v) => v[0],
            Err((_, r)) => {
                device.destroy_shader_module(vert_module, None);
                device.destroy_shader_module(frag_module, None);
                device.destroy_pipeline_layout(layout, None);
                return Err(r);
            }
        };

        device.destroy_shader_module(vert_module, None);

        Ok(UserPipeline {
            id,
            pipeline,
            module: frag_module,
            layout,
            owns_layout: true,
        })
    }
}

fn slice_to_u32(blob: &[u8]) -> Vec<u32> {
    blob.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}