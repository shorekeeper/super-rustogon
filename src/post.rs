//! Post-process stage.
//!
//! The engine renders the scene into an offscreen color target
//! instead of directly into the swapchain. A second render pass
//! then samples that target through a fragment shader (`post.frag`)
//! and writes the final image into the swapchain. This is the
//! piece that realises the "bloom / vignette / chromatic / scanlines
//! / film grain / colorblind / high-contrast" settings the config
//! and options UI expose.
//!
//! The module owns every Vulkan object involved:
//!
//! * one offscreen color image + memory + view + framebuffer,
//! * the offscreen render pass (target for the main pipeline),
//! * the post render pass (target for the swapchain),
//! * one swapchain framebuffer per swapchain image,
//! * the post graphics pipeline (fullscreen triangle),
//! * a descriptor set layout / pool / set binding the offscreen
//!   view as a combined image sampler.
//!
//! `PostStage::offscreen_render_pass()` is the render pass the
//! main pipeline must be created with; compatibility between the
//! two is enforced by sharing the exact same attachment format.
//!
//! Resize handling: [`PostStage::recreate`] rebuilds every object
//! whose dimensions follow the swapchain (image, framebuffers)
//! and rewrites the descriptor set so the sampler points at the
//! freshly created view. Pipelines, render passes, sampler and
//! descriptor layouts survive resizes.

use ash::{vk, Device, Instance};
use std::ffi::CStr;

const POST_VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/post.vert.spv"));
const POST_FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/post.frag.spv"));

/// Push-constant block fed to `post.frag`. Layout must match the
/// GLSL `Params` uniform byte for byte; the struct is `repr(C)`
/// so the Rust compiler cannot reorder fields.
///
/// Total size: 48 bytes. Well under the 128 byte minimum Vulkan
/// guarantees for push-constant storage on every implementation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PostParams {
    pub time:               f32,
    pub bloom_intensity:    f32,
    pub chromatic_strength: f32,
    pub vignette:           f32,
    pub scanlines:          f32,
    pub film_grain:         f32,
    pub colorblind_mode:    f32,
    pub high_contrast:      f32,
    pub resolution:         [f32; 2],
    pub glitch:             f32,
    pub strobe:             f32,
    // v3 trigger extensions. Kept at the tail so the earlier
    // field offsets match the original layout byte for byte.
    pub invert_colors:      f32,
    pub grayscale:          f32,
    pub shockwave_progress: f32,
    pub shockwave_strength: f32,
    pub fog_near:           f32,
    pub fog_far:            f32,
    pub outline_amount:     f32,
    pub _pad:               f32,
}

impl Default for PostParams {
    fn default() -> Self {
        PostParams {
            time: 0.0,
            bloom_intensity: 0.0,
            chromatic_strength: 0.0,
            vignette: 0.0,
            scanlines: 0.0,
            film_grain: 0.0,
            colorblind_mode: 0.0,
            high_contrast: 0.0,
            resolution: [1.0, 1.0],
            glitch: 0.0,
            strobe: 0.0,
            invert_colors: 0.0,
            grayscale: 0.0,
            shockwave_progress: 0.0,
            shockwave_strength: 0.0,
            fog_near: 0.0,
            fog_far: 0.0,
            outline_amount: 0.0,
            _pad: 0.0,
        }
    }
}

pub struct PostStage {
    // ---- offscreen target (written by the main pipeline) ----
    offscreen_image:       vk::Image,
    offscreen_memory:      vk::DeviceMemory,
    offscreen_view:        vk::ImageView,
    offscreen_framebuffer: vk::Framebuffer,
    offscreen_render_pass: vk::RenderPass,
    offscreen_format:      vk::Format,

    // ---- post target (written to the swapchain) ----
    post_render_pass:  vk::RenderPass,
    post_framebuffers: Vec<vk::Framebuffer>,

    // ---- post pipeline ----
    pipeline:        vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,

    // ---- descriptor binding the offscreen view as a sampler ----
    sampler:               vk::Sampler,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool:       vk::DescriptorPool,
    descriptor_set:        vk::DescriptorSet,

    extent: vk::Extent2D,
}

impl PostStage {
    /// Build the whole post-process stage. `swapchain_views` is
    /// the per-image view list the renderer already owns; we do
    /// not take ownership, we only read them to build one
    /// framebuffer per image.
    pub fn new(
        instance:         &Instance,
        physical_device:  vk::PhysicalDevice,
        device:           &Device,
        swapchain_format: vk::Format,
        swapchain_views:  &[vk::ImageView],
        extent:           vk::Extent2D,
    ) -> Self {
        unsafe {
            // R8G8B8A8_UNORM is required to be supported as both
            // COLOR_ATTACHMENT and SAMPLED on every Vulkan
            // implementation, so we do not need a format query.
            let offscreen_format = vk::Format::R8G8B8A8_UNORM;

            let offscreen_render_pass =
                build_offscreen_render_pass(device, offscreen_format);
            let post_render_pass =
                build_post_render_pass(device, swapchain_format);

            let sampler = build_sampler(device);
            let descriptor_set_layout = build_descriptor_set_layout(device);
            let descriptor_pool = build_descriptor_pool(device);
            let descriptor_set = allocate_descriptor_set(
                device, descriptor_pool, descriptor_set_layout);

            let pipeline_layout =
                build_pipeline_layout(device, descriptor_set_layout);
            let pipeline = build_pipeline(
                device, post_render_pass, pipeline_layout);

            let (offscreen_image, offscreen_memory, offscreen_view) =
                build_offscreen_target(
                    instance, physical_device, device,
                    offscreen_format, extent);

            let offscreen_framebuffer = build_offscreen_framebuffer(
                device, offscreen_render_pass, offscreen_view, extent);

            let post_framebuffers = build_post_framebuffers(
                device, post_render_pass, swapchain_views, extent);

            update_descriptor_set(
                device, descriptor_set, offscreen_view, sampler);

            PostStage {
                offscreen_image, offscreen_memory, offscreen_view,
                offscreen_framebuffer,
                offscreen_render_pass,
                offscreen_format,
                post_render_pass,
                post_framebuffers,
                pipeline, pipeline_layout,
                sampler,
                descriptor_set_layout,
                descriptor_pool,
                descriptor_set,
                extent,
            }
        }
    }

    /// Render pass the main (scene) pipeline must be compatible
    /// with. Exposed so `crate::pipeline::Pipeline::new` can hook
    /// into it without the renderer having to duplicate the pass.
    pub fn offscreen_render_pass(&self) -> vk::RenderPass {
        self.offscreen_render_pass
    }

    /// Rebuild everything that depends on the swapchain extent.
    /// Render passes, descriptor layout/pool/set, pipeline, sampler
    /// are kept; only sized resources are torn down and re-made.
    /// The descriptor set is rewritten to reference the new
    /// offscreen view.
    pub fn recreate(
        &mut self,
        instance:        &Instance,
        physical_device: vk::PhysicalDevice,
        device:          &Device,
        swapchain_views: &[vk::ImageView],
        extent:          vk::Extent2D,
    ) {
        unsafe {
            device.device_wait_idle().ok();

            // Tear down sized resources.
            for &fb in &self.post_framebuffers {
                device.destroy_framebuffer(fb, None);
            }
            self.post_framebuffers.clear();
            device.destroy_framebuffer(self.offscreen_framebuffer, None);
            device.destroy_image_view(self.offscreen_view, None);
            device.destroy_image(self.offscreen_image, None);
            device.free_memory(self.offscreen_memory, None);

            // Recreate at the new size.
            let (img, mem, view) = build_offscreen_target(
                instance, physical_device, device,
                self.offscreen_format, extent);
            self.offscreen_image = img;
            self.offscreen_memory = mem;
            self.offscreen_view   = view;

            self.offscreen_framebuffer = build_offscreen_framebuffer(
                device, self.offscreen_render_pass, self.offscreen_view, extent);
            self.post_framebuffers = build_post_framebuffers(
                device, self.post_render_pass, swapchain_views, extent);

            update_descriptor_set(
                device, self.descriptor_set, self.offscreen_view, self.sampler);

            self.extent = extent;
        }
    }

    /// Begin the offscreen render pass. The caller is expected
    /// to bind the main pipeline, set viewport/scissor/push
    /// constants and draw, then call [`end_render_pass`].
    pub fn begin_offscreen(
        &self, device: &Device, cmd: vk::CommandBuffer, extent: vk::Extent2D,
    ) {
        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] },
        }];
        let rp_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.offscreen_render_pass)
            .framebuffer(self.offscreen_framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .clear_values(&clear_values);
        unsafe {
            device.cmd_begin_render_pass(
                cmd, &rp_begin, vk::SubpassContents::INLINE);
        }
    }

    /// End the current render pass (either offscreen or post).
    pub fn end_render_pass(&self, device: &Device, cmd: vk::CommandBuffer) {
        unsafe { device.cmd_end_render_pass(cmd); }
    }

    /// Full post pass: begin render pass, bind pipeline / descriptor,
    /// push post params, draw the fullscreen triangle, end pass.
    /// `image_index` is the swapchain image index returned by
    /// `acquire_next_image`.
    pub fn render_post(
        &self,
        device:      &Device,
        cmd:         vk::CommandBuffer,
        image_index: u32,
        extent:      vk::Extent2D,
        params:      &PostParams,
    ) {
        unsafe {
            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] },
            }];
            let rp_begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.post_render_pass)
                .framebuffer(self.post_framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent,
                })
                .clear_values(&clear_values);
            device.cmd_begin_render_pass(
                cmd, &rp_begin, vk::SubpassContents::INLINE);

            let viewport = vk::Viewport {
                x: 0.0, y: 0.0,
                width:  extent.width  as f32,
                height: extent.height as f32,
                min_depth: 0.0, max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 }, extent,
            };
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor (cmd, 0, &[scissor]);

            device.cmd_bind_pipeline(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout, 0, &[self.descriptor_set], &[]);

            let bytes = std::slice::from_raw_parts(
                (params as *const PostParams) as *const u8,
                std::mem::size_of::<PostParams>(),
            );
            device.cmd_push_constants(
                cmd, self.pipeline_layout,
                vk::ShaderStageFlags::FRAGMENT, 0, bytes);

            device.cmd_draw(cmd, 3, 1, 0, 0);

            device.cmd_end_render_pass(cmd);
        }
    }

    /// Release every Vulkan object. Must be called while the
    /// device is idle (the renderer owns the device and ensures
    /// this in its own destroy path).
    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            for &fb in &self.post_framebuffers {
                device.destroy_framebuffer(fb, None);
            }
            self.post_framebuffers.clear();
            device.destroy_framebuffer(self.offscreen_framebuffer, None);
            device.destroy_image_view(self.offscreen_view, None);
            device.destroy_image(self.offscreen_image, None);
            device.free_memory(self.offscreen_memory, None);

            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);

            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.sampler, None);

            device.destroy_render_pass(self.post_render_pass, None);
            device.destroy_render_pass(self.offscreen_render_pass, None);
        }
    }
    
    /// Render pass the swapchain post pipeline is built
    /// against. User shaders from the sandbox must be
    /// compiled against this pass so the resulting pipeline
    /// can be swapped in for the default one.
    pub fn post_render_pass(&self) -> vk::RenderPass {
        self.post_render_pass
    }

    /// Descriptor set layout used by the default post
    /// pipeline. Exposed so the sandbox can build a user
    /// pipeline with a compatible layout and the sandbox
    /// pipeline can reuse the exact same descriptor set
    /// the built in pipeline already has bound to the
    /// offscreen target.
    pub fn descriptor_set_layout(&self) -> vk::DescriptorSetLayout {
        self.descriptor_set_layout
    }

    /// Swapchain image count, for sandbox bench rigs that
    /// want to allocate per frame resources. Currently
    /// unused by the shipped sandbox but stable enough to
    /// be part of the public surface.
    pub fn framebuffer_count(&self) -> usize {
        self.post_framebuffers.len()
    }

    /// Variant of `render_post` that runs a user supplied
    /// pipeline instead of the built in one. The caller is
    /// responsible for:
    ///
    /// * Providing a pipeline compatible with
    ///   `post_render_pass()` and `descriptor_set_layout()`.
    /// * Providing push constant bytes whose length does not
    ///   exceed the user pipeline's declared range. A short
    ///   slice is padded with zeros, a longer one is
    ///   truncated, which matches the "best effort" policy
    ///   of the rest of the renderer.
    ///
    /// The descriptor set from the built in pipeline is
    /// bound automatically, so the user shader samples the
    /// same offscreen scene texture as the default post.
    pub fn render_post_with_pipeline(
        &self,
        device:           &Device,
        cmd:              vk::CommandBuffer,
        image_index:      u32,
        extent:           vk::Extent2D,
        custom_pipeline:  vk::Pipeline,
        custom_layout:    vk::PipelineLayout,
        push_bytes:       &[u8],
    ) {
        unsafe {
            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] },
            }];
            let rp_begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.post_render_pass)
                .framebuffer(self.post_framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent,
                })
                .clear_values(&clear_values);
            device.cmd_begin_render_pass(
                cmd, &rp_begin, vk::SubpassContents::INLINE);

            let viewport = vk::Viewport {
                x: 0.0, y: 0.0,
                width:  extent.width  as f32,
                height: extent.height as f32,
                min_depth: 0.0, max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 }, extent,
            };
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor (cmd, 0, &[scissor]);

            device.cmd_bind_pipeline(
                cmd, vk::PipelineBindPoint::GRAPHICS, custom_pipeline);
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS,
                custom_layout, 0, &[self.descriptor_set], &[]);

            // Push constants. Pad or truncate to 128 bytes
            // to match the user pipeline's declared range,
            // then push as a single call.
            if !push_bytes.is_empty() {
                let mut padded = [0u8; 128];
                let n = push_bytes.len().min(padded.len());
                padded[..n].copy_from_slice(&push_bytes[..n]);
                device.cmd_push_constants(
                    cmd, custom_layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    0, &padded[..]);
            }

            device.cmd_draw(cmd, 3, 1, 0, 0);

            device.cmd_end_render_pass(cmd);
        }
    }
}

// ---------- builders ----------

/// Offscreen render pass. One COLOR attachment, CLEAR on load,
/// STORE on end, final layout SHADER_READ_ONLY so the post pass
/// can sample from it with no extra barrier.
///
/// Two subpass dependencies:
///
/// * external -> 0 at FRAGMENT_SHADER, so the previous frame's
///   post read finishes before this frame's write.
/// * 0 -> external from COLOR_ATTACHMENT_OUTPUT to FRAGMENT_SHADER,
///   so this frame's write is visible to the post pass that
///   consumes it in the very same command buffer.
unsafe fn build_offscreen_render_pass(
    device: &Device, format: vk::Format,
) -> vk::RenderPass {
    let attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

    let color_ref = [vk::AttachmentReference {
        attachment: 0,
        layout:     vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }];

    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_ref);

    let deps = [
        vk::SubpassDependency {
            src_subpass:      vk::SUBPASS_EXTERNAL,
            dst_subpass:      0,
            src_stage_mask:   vk::PipelineStageFlags::FRAGMENT_SHADER,
            dst_stage_mask:   vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            src_access_mask:  vk::AccessFlags::SHADER_READ,
            dst_access_mask:  vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            dependency_flags: vk::DependencyFlags::empty(),
        },
        vk::SubpassDependency {
            src_subpass:      0,
            dst_subpass:      vk::SUBPASS_EXTERNAL,
            src_stage_mask:   vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            dst_stage_mask:   vk::PipelineStageFlags::FRAGMENT_SHADER,
            src_access_mask:  vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            dst_access_mask:  vk::AccessFlags::SHADER_READ,
            dependency_flags: vk::DependencyFlags::empty(),
        },
    ];

    let attachments = [attachment];
    let subpasses   = [subpass];

    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&deps);

    device.create_render_pass(&info, None).unwrap()
}

/// Post render pass. Targets the swapchain. Same structure as the
/// original `build_render_pass` used to have inside renderer.rs
/// before the post stage existed.
unsafe fn build_post_render_pass(
    device: &Device, format: vk::Format,
) -> vk::RenderPass {
    let attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);

    let color_ref = [vk::AttachmentReference {
        attachment: 0,
        layout:     vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }];

    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_ref);

    let dep = vk::SubpassDependency {
        src_subpass:      vk::SUBPASS_EXTERNAL,
        dst_subpass:      0,
        src_stage_mask:   vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        dst_stage_mask:   vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        src_access_mask:  vk::AccessFlags::empty(),
        dst_access_mask:  vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        dependency_flags: vk::DependencyFlags::empty(),
    };

    let attachments = [attachment];
    let subpasses   = [subpass];
    let deps        = [dep];

    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&deps);

    device.create_render_pass(&info, None).unwrap()
}

unsafe fn build_sampler(device: &Device) -> vk::Sampler {
    let info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .min_lod(0.0)
        .max_lod(0.0);
    device.create_sampler(&info, None).unwrap()
}

unsafe fn build_descriptor_set_layout(device: &Device) -> vk::DescriptorSetLayout {
    let bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    device.create_descriptor_set_layout(&info, None).unwrap()
}

unsafe fn build_descriptor_pool(device: &Device) -> vk::DescriptorPool {
    let sizes = [vk::DescriptorPoolSize {
        ty:              vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        descriptor_count: 1,
    }];
    let info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(1)
        .pool_sizes(&sizes);
    device.create_descriptor_pool(&info, None).unwrap()
}

unsafe fn allocate_descriptor_set(
    device: &Device,
    pool:   vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
) -> vk::DescriptorSet {
    let layouts = [layout];
    let info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    device.allocate_descriptor_sets(&info).unwrap()[0]
}

unsafe fn update_descriptor_set(
    device:  &Device,
    set:     vk::DescriptorSet,
    view:    vk::ImageView,
    sampler: vk::Sampler,
) {
    let img = [vk::DescriptorImageInfo {
        sampler,
        image_view:   view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    }];
    let write = [vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(&img)];
    device.update_descriptor_sets(&write, &[]);
}

unsafe fn build_pipeline_layout(
    device: &Device, set_layout: vk::DescriptorSetLayout,
) -> vk::PipelineLayout {
    let set_layouts = [set_layout];
    let ranges = [vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        .offset(0)
        .size(std::mem::size_of::<PostParams>() as u32)];
    let info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(&set_layouts)
        .push_constant_ranges(&ranges);
    device.create_pipeline_layout(&info, None).unwrap()
}

unsafe fn build_pipeline(
    device: &Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert = create_shader_module(device, POST_VERT_SPV);
    let frag = create_shader_module(device, POST_FRAG_SPV);
    let entry = CStr::from_bytes_with_nul(b"main\0").unwrap();

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert).name(entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag).name(entry),
    ];

    // No vertex input. Fullscreen triangle generates positions
    // from gl_VertexIndex inside the shader.
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

    // Opaque write. The post pass replaces the swapchain color
    // entirely; no blending needed.
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

    let pipeline = device.create_graphics_pipelines(
        vk::PipelineCache::null(), &[info], None,
    ).expect("vkCreateGraphicsPipelines(post) failed")[0];

    device.destroy_shader_module(vert, None);
    device.destroy_shader_module(frag, None);

    pipeline
}

unsafe fn create_shader_module(device: &Device, code: &[u8]) -> vk::ShaderModule {
    assert!(code.len() % 4 == 0, "SPIR-V blob length must be a multiple of 4");
    let words: Vec<u32> = code.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    device.create_shader_module(&info, None).unwrap()
}

unsafe fn build_offscreen_target(
    instance:        &Instance,
    physical_device: vk::PhysicalDevice,
    device:          &Device,
    format:          vk::Format,
    extent:          vk::Extent2D,
) -> (vk::Image, vk::DeviceMemory, vk::ImageView) {
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .format(format)
        .tiling(vk::ImageTiling::OPTIMAL)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
        .samples(vk::SampleCountFlags::TYPE_1)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let image = device.create_image(&image_info, None).unwrap();

    let req = device.get_image_memory_requirements(image);
    let props = instance.get_physical_device_memory_properties(physical_device);
    let idx = find_memory_type(
        &props, req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL);
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(req.size)
        .memory_type_index(idx);
    let memory = device.allocate_memory(&alloc, None).unwrap();
    device.bind_image_memory(image, memory, 0).unwrap();

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .components(vk::ComponentMapping::default())
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask:      vk::ImageAspectFlags::COLOR,
            base_mip_level:   0, level_count: 1,
            base_array_layer: 0, layer_count: 1,
        });
    let view = device.create_image_view(&view_info, None).unwrap();
    (image, memory, view)
}

unsafe fn build_offscreen_framebuffer(
    device:      &Device,
    render_pass: vk::RenderPass,
    view:        vk::ImageView,
    extent:      vk::Extent2D,
) -> vk::Framebuffer {
    let attachments = [view];
    let info = vk::FramebufferCreateInfo::default()
        .render_pass(render_pass)
        .attachments(&attachments)
        .width(extent.width).height(extent.height).layers(1);
    device.create_framebuffer(&info, None).unwrap()
}

unsafe fn build_post_framebuffers(
    device:      &Device,
    render_pass: vk::RenderPass,
    views:       &[vk::ImageView],
    extent:      vk::Extent2D,
) -> Vec<vk::Framebuffer> {
    views.iter().map(|&v| {
        let attachments = [v];
        let info = vk::FramebufferCreateInfo::default()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width).height(extent.height).layers(1);
        device.create_framebuffer(&info, None).unwrap()
    }).collect()
}

fn find_memory_type(
    props:     &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    flags:     vk::MemoryPropertyFlags,
) -> u32 {
    for i in 0..props.memory_type_count {
        let suitable = (type_bits & (1 << i)) != 0
            && props.memory_types[i as usize].property_flags.contains(flags);
        if suitable { return i; }
    }
    panic!("No memory type satisfying {:?}", flags);
}