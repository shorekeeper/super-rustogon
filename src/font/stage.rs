//! Vulkan integration of the font atlas.
//!
//! Owns the atlas image, sampler, descriptor set, graphics
//! pipeline, and per frame vertex buffers. Exposes a single
//! `render` method that the renderer calls inside the offscreen
//! pass after the main shape draw.
//!
//! The text pipeline is fully separate from the main pipeline:
//! it has its own shader modules, its own vertex format
//! (`TextVertex` carries UV coordinates whereas `Vertex` does
//! not), and its own descriptor set bound to the atlas. There
//! is no interaction between the two pipelines beyond rendering
//! into the same offscreen color attachment in render order.
//!
//! # Atlas upload
//!
//! Performed once at construction time through a one shot
//! command buffer. The atlas pixels are first copied to a host
//! visible staging buffer, then transitioned through standard
//! image layout barriers to SHADER_READ_ONLY_OPTIMAL via
//! vkCmdCopyBufferToImage. The staging buffer is destroyed
//! after the queue idle wait completes.

use ash::{vk, Device, Instance};
use std::ffi::CStr;
use std::ptr;

use crate::font::bake::FontAtlas;

const TEXT_VERT_SPV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/text.vert.spv"));
const TEXT_FRAG_SPV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/text.frag.spv"));

const MAX_FRAMES_IN_FLIGHT: usize = 2;
const VERTEX_CAPACITY: usize = 65_536;

/// Vertex format for SDF text. Position is in game space,
/// matching the existing `crate::pipeline::Vertex`. UV is in
/// 0..1 atlas space. Color is RGBA, where alpha multiplies the
/// SDF coverage in the fragment shader.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TextVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl TextVertex {
    pub fn new(pos: [f32; 2], uv: [f32; 2], color: [f32; 4]) -> Self {
        TextVertex { pos, uv, color }
    }
}

/// Push constant block. Matches `text.vert` byte for byte.
#[repr(C)]
#[derive(Clone, Copy)]
struct PushConstants {
    scale: [f32; 2],
}

/// Per frame vertex buffer. Persistently mapped, written by the
/// CPU on each frame and consumed by the GPU on the same frame
/// (synchronization is the renderer's per frame fence).
struct TextBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut TextVertex,
    capacity: usize,
}

impl TextBuffer {
    unsafe fn new(
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        device: &Device,
        capacity: usize,
    ) -> Self {
        let bytes = (capacity * std::mem::size_of::<TextVertex>()) as vk::DeviceSize;
        let info = vk::BufferCreateInfo::default()
            .size(bytes)
            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = device.create_buffer(&info, None).unwrap();

        let req = device.get_buffer_memory_requirements(buffer);
        let props = instance.get_physical_device_memory_properties(physical_device);
        let idx = find_memory_type(
            &props,
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE
                | vk::MemoryPropertyFlags::HOST_COHERENT,
        );
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(idx);
        let memory = device.allocate_memory(&alloc, None).unwrap();
        device.bind_buffer_memory(buffer, memory, 0).unwrap();

        let mapped = device
            .map_memory(memory, 0, bytes, vk::MemoryMapFlags::empty())
            .unwrap() as *mut TextVertex;

        TextBuffer { buffer, memory, mapped, capacity }
    }

    unsafe fn upload(&mut self, vertices: &[TextVertex]) -> usize {
        let n = vertices.len().min(self.capacity);
        if n > 0 {
            ptr::copy_nonoverlapping(vertices.as_ptr(), self.mapped, n);
        }
        n
    }

    unsafe fn destroy(&self, device: &Device) {
        device.unmap_memory(self.memory);
        device.destroy_buffer(self.buffer, None);
        device.free_memory(self.memory, None);
    }
}

/// Vulkan resources for SDF text rendering.
pub struct TextStage {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    sampler: vk::Sampler,

    atlas_image: vk::Image,
    atlas_memory: vk::DeviceMemory,
    atlas_view: vk::ImageView,

    buffers: Vec<TextBuffer>,
    current_frame: usize,
}

impl TextStage {
    /// Build the entire text rendering subsystem from a baked
    /// atlas plus the Vulkan handles owned by the renderer.
    /// `render_pass` must be the offscreen render pass the main
    /// shape pipeline draws into; the text pipeline is built
    /// against the same pass so both can run inside one
    /// `begin/end` block.
    pub fn new(
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        device: &Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        atlas: &FontAtlas,
    ) -> Self {
        unsafe {
            let (atlas_image, atlas_memory, atlas_view) = build_atlas_image(
                instance, physical_device, device, queue, command_pool, atlas,
            );
            let sampler = build_sampler(device);
            let descriptor_set_layout = build_descriptor_set_layout(device);
            let descriptor_pool = build_descriptor_pool(device);
            let descriptor_set = allocate_descriptor_set(
                device, descriptor_pool, descriptor_set_layout);
            update_descriptor_set(device, descriptor_set, atlas_view, sampler);

            let pipeline_layout = build_pipeline_layout(device, descriptor_set_layout);
            let pipeline = build_pipeline(device, render_pass, pipeline_layout);

            let mut buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
            for _ in 0..MAX_FRAMES_IN_FLIGHT {
                buffers.push(TextBuffer::new(
                    instance, physical_device, device, VERTEX_CAPACITY));
            }

            TextStage {
                pipeline, pipeline_layout,
                descriptor_set_layout,
                descriptor_pool, descriptor_set,
                sampler,
                atlas_image, atlas_memory, atlas_view,
                buffers,
                current_frame: 0,
            }
        }
    }

    /// Append a text draw to the current command buffer.
    ///
    /// Called by the renderer between `begin_offscreen` and
    /// `end_render_pass` of the offscreen pass, after the main
    /// pipeline draw. The renderer is responsible for resetting
    /// viewport and scissor before this call; we only bind
    /// pipeline, descriptor set, vertex buffer, push constants,
    /// and issue a single draw.
    pub fn render(
        &mut self,
        device: &Device,
        cmd: vk::CommandBuffer,
        vertices: &[TextVertex],
        scale: [f32; 2],
    ) {
        if vertices.is_empty() { return; }

        let n = unsafe {
            self.buffers[self.current_frame].upload(vertices)
        };
        if n == 0 { return; }

        unsafe {
            device.cmd_bind_pipeline(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout, 0,
                &[self.descriptor_set], &[],
            );
            device.cmd_bind_vertex_buffers(
                cmd, 0,
                &[self.buffers[self.current_frame].buffer],
                &[0],
            );

            let pc = PushConstants { scale };
            let bytes = std::slice::from_raw_parts(
                &pc as *const PushConstants as *const u8,
                std::mem::size_of::<PushConstants>(),
            );
            device.cmd_push_constants(
                cmd, self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX, 0, bytes);

            device.cmd_draw(cmd, n as u32, 1, 0, 0);
        }

        self.current_frame = (self.current_frame + 1) % MAX_FRAMES_IN_FLIGHT;
    }

    /// Release every Vulkan object. The caller must ensure the
    /// device is idle.
    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            for b in &self.buffers { b.destroy(device); }
            self.buffers.clear();

            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);

            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.sampler, None);

            device.destroy_image_view(self.atlas_view, None);
            device.destroy_image(self.atlas_image, None);
            device.free_memory(self.atlas_memory, None);
        }
    }
}

// Builders below mirror the patterns used in
// `crate::pipeline::Pipeline` and `crate::post::PostStage`.

unsafe fn build_atlas_image(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    device: &Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    atlas: &FontAtlas,
) -> (vk::Image, vk::DeviceMemory, vk::ImageView) {
    let format = vk::Format::R8_UNORM;
    let extent = vk::Extent3D {
        width: atlas.width, height: atlas.height, depth: 1,
    };

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(extent)
        .mip_levels(1).array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = device.create_image(&image_info, None).unwrap();

    let req = device.get_image_memory_requirements(image);
    let props = instance.get_physical_device_memory_properties(physical_device);
    let idx = find_memory_type(
        &props, req.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    );
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(req.size)
        .memory_type_index(idx);
    let memory = device.allocate_memory(&alloc, None).unwrap();
    device.bind_image_memory(image, memory, 0).unwrap();

    // Staging buffer.
    let pixel_bytes = atlas.pixels.len() as vk::DeviceSize;
    let staging_info = vk::BufferCreateInfo::default()
        .size(pixel_bytes)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let staging = device.create_buffer(&staging_info, None).unwrap();

    let staging_req = device.get_buffer_memory_requirements(staging);
    let staging_idx = find_memory_type(
        &props, staging_req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE
            | vk::MemoryPropertyFlags::HOST_COHERENT,
    );
    let staging_alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(staging_req.size)
        .memory_type_index(staging_idx);
    let staging_mem = device.allocate_memory(&staging_alloc, None).unwrap();
    device.bind_buffer_memory(staging, staging_mem, 0).unwrap();

    let map = device
        .map_memory(staging_mem, 0, pixel_bytes, vk::MemoryMapFlags::empty())
        .unwrap();
    ptr::copy_nonoverlapping(
        atlas.pixels.as_ptr(),
        map as *mut u8,
        atlas.pixels.len(),
    );
    device.unmap_memory(staging_mem);

    // One shot command buffer for layout transitions and copy.
    let cb_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cb = device.allocate_command_buffers(&cb_info).unwrap()[0];

    let begin = vk::CommandBufferBeginInfo::default()
        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    device.begin_command_buffer(cb, &begin).unwrap();

    let to_dst = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0, level_count: 1,
            base_array_layer: 0, layer_count: 1,
        });
    device.cmd_pipeline_barrier(
        cb,
        vk::PipelineStageFlags::TOP_OF_PIPE,
        vk::PipelineStageFlags::TRANSFER,
        vk::DependencyFlags::empty(),
        &[], &[], &[to_dst],
    );

    let region = vk::BufferImageCopy::default()
        .buffer_offset(0)
        .buffer_row_length(0)
        .buffer_image_height(0)
        .image_subresource(vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0, layer_count: 1,
        })
        .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
        .image_extent(extent);
    device.cmd_copy_buffer_to_image(
        cb, staging, image,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        &[region],
    );

    let to_shader = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::SHADER_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0, level_count: 1,
            base_array_layer: 0, layer_count: 1,
        });
    device.cmd_pipeline_barrier(
        cb,
        vk::PipelineStageFlags::TRANSFER,
        vk::PipelineStageFlags::FRAGMENT_SHADER,
        vk::DependencyFlags::empty(),
        &[], &[], &[to_shader],
    );

    device.end_command_buffer(cb).unwrap();

    // The submit info borrows from the command buffer slice,
    // so the slice must outlive the SubmitInfo struct. A bare
    // `&[cb]` literal would be a temporary dropped before the
    // queue_submit call reads it.
    let cmd_bufs = [cb];
    let submit = vk::SubmitInfo::default().command_buffers(&cmd_bufs);
    device.queue_submit(queue, &[submit], vk::Fence::null()).unwrap();
    device.queue_wait_idle(queue).unwrap();

    device.free_command_buffers(command_pool, &[cb]);
    device.destroy_buffer(staging, None);
    device.free_memory(staging_mem, None);

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .components(vk::ComponentMapping {
            r: vk::ComponentSwizzle::R,
            g: vk::ComponentSwizzle::R,
            b: vk::ComponentSwizzle::R,
            a: vk::ComponentSwizzle::ONE,
        })
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0, level_count: 1,
            base_array_layer: 0, layer_count: 1,
        });
    let view = device.create_image_view(&view_info, None).unwrap();

    (image, memory, view)
}

unsafe fn build_sampler(device: &Device) -> vk::Sampler {
    let info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .min_lod(0.0).max_lod(0.0);
    device.create_sampler(&info, None).unwrap()
}

unsafe fn build_descriptor_set_layout(device: &Device) -> vk::DescriptorSetLayout {
    let bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let info = vk::DescriptorSetLayoutCreateInfo::default()
        .bindings(&bindings);
    device.create_descriptor_set_layout(&info, None).unwrap()
}

unsafe fn build_descriptor_pool(device: &Device) -> vk::DescriptorPool {
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        descriptor_count: 1,
    }];
    let info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(1)
        .pool_sizes(&sizes);
    device.create_descriptor_pool(&info, None).unwrap()
}

unsafe fn allocate_descriptor_set(
    device: &Device,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
) -> vk::DescriptorSet {
    let layouts = [layout];
    let info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    device.allocate_descriptor_sets(&info).unwrap()[0]
}

unsafe fn update_descriptor_set(
    device: &Device,
    set: vk::DescriptorSet,
    view: vk::ImageView,
    sampler: vk::Sampler,
) {
    let img = [vk::DescriptorImageInfo {
        sampler,
        image_view: view,
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
    device: &Device,
    set_layout: vk::DescriptorSetLayout,
) -> vk::PipelineLayout {
    let set_layouts = [set_layout];
    let ranges = [vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::VERTEX)
        .offset(0)
        .size(std::mem::size_of::<PushConstants>() as u32)];
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
    let vert = create_shader_module(device, TEXT_VERT_SPV);
    let frag = create_shader_module(device, TEXT_FRAG_SPV);
    let entry = CStr::from_bytes_with_nul(b"main\0").unwrap();

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert).name(entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag).name(entry),
    ];

    let bindings = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<TextVertex>() as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];
    let attrs = [
        vk::VertexInputAttributeDescription {
            location: 0, binding: 0,
            format: vk::Format::R32G32_SFLOAT, offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1, binding: 0,
            format: vk::Format::R32G32_SFLOAT, offset: 8,
        },
        vk::VertexInputAttributeDescription {
            location: 2, binding: 0,
            format: vk::Format::R32G32B32A32_SFLOAT, offset: 16,
        },
    ];
    let vi = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attrs);

    let ia = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

    let viewport = vk::PipelineViewportStateCreateInfo::default()
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
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default()
        .attachments(&blend_attach);

    let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default()
        .dynamic_states(&dyn_states);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vi)
        .input_assembly_state(&ia)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&ms)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);

    let pipeline = device
        .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
        .expect("vkCreateGraphicsPipelines(text) failed")[0];

    device.destroy_shader_module(vert, None);
    device.destroy_shader_module(frag, None);

    pipeline
}

unsafe fn create_shader_module(device: &Device, code: &[u8]) -> vk::ShaderModule {
    assert!(code.len() % 4 == 0,
        "SPIR-V blob length must be a multiple of 4");
    let words: Vec<u32> = code
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    device.create_shader_module(&info, None).unwrap()
}

fn find_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> u32 {
    for i in 0..props.memory_type_count {
        let suitable = (type_bits & (1 << i)) != 0
            && props.memory_types[i as usize].property_flags.contains(flags);
        if suitable {
            return i;
        }
    }
    panic!("no memory type satisfies {:?}", flags);
}