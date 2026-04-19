//! Vulkan renderer skeleton.
//!
//! Responsibilities at this stage:
//!
//! * Load the Vulkan loader at runtime (no link time dependency on
//!   `vulkan-1.lib`).
//! * Create instance, surface, logical device, queue, swapchain,
//!   render pass, framebuffers, command pool, command buffers and
//!   per frame sync primitives.
//! * Re-create the swapchain when the window is resized or when the
//!   driver reports the chain is out of date / suboptimal.
//! * Issue one render pass per frame that clears the full surface to
//!   a background color and then clears a centered square sub-region
//!   to a different color, so the user can see immediately whether
//!   the chosen aspect ratio gives a sensible playfield.
//!
//! Notably absent on purpose: any pipelines, shaders, vertex buffers,
//! descriptor sets, or game logic. Adding them later does not require
//! restructuring anything in this file beyond `render_frame`.

use ash::{vk, Device, Entry, Instance};
use std::ffi::{c_void, CString};

/// How many frames may be in flight on the GPU simultaneously. Two is a
/// good compromise between latency and CPU/GPU overlap for a fast paced
/// game like this one.
const MAX_FRAMES_IN_FLIGHT: usize = 2;

pub struct Renderer {
    _entry:           Entry,
    instance:         Instance,
    surface_loader:   ash::khr::surface::Instance,
    surface:          vk::SurfaceKHR,

    physical_device:  vk::PhysicalDevice,
    queue_family:     u32,
    device:           Device,
    queue:            vk::Queue,

    swapchain_loader: ash::khr::swapchain::Device,
    swapchain:        vk::SwapchainKHR,
    swapchain_format: vk::Format,
    swapchain_extent: vk::Extent2D,
    swapchain_views:  Vec<vk::ImageView>,

    render_pass:      vk::RenderPass,
    framebuffers:     Vec<vk::Framebuffer>,

    command_pool:     vk::CommandPool,
    command_buffers:  Vec<vk::CommandBuffer>,

    image_available:  [vk::Semaphore; MAX_FRAMES_IN_FLIGHT],
    render_finished:  [vk::Semaphore; MAX_FRAMES_IN_FLIGHT],
    in_flight:        [vk::Fence;     MAX_FRAMES_IN_FLIGHT],
    current_frame:    usize,
}

impl Renderer {
    /// Bring up Vulkan and produce a fully usable renderer bound to the
    /// supplied Win32 window. Panics on any unrecoverable setup failure;
    /// for a skeleton this is acceptable, real shipping code would
    /// surface these errors to the user.
    pub fn new(hinstance: *mut c_void, hwnd: *mut c_void) -> Self {
        unsafe {
            let entry = Entry::load().expect("Vulkan loader not present");

            // Instance with the two surface extensions required to talk
            // to a Win32 HWND.
            let app_name = CString::new("HexRS").unwrap();
            let app_info = vk::ApplicationInfo::default()
                .application_name(&app_name)
                .application_version(vk::make_api_version(0, 0, 1, 0))
                .engine_name(&app_name)
                .engine_version(vk::make_api_version(0, 0, 1, 0))
                .api_version(vk::API_VERSION_1_0);

            let inst_exts = [
                ash::khr::surface::NAME.as_ptr(),
                ash::khr::win32_surface::NAME.as_ptr(),
            ];
            let inst_info = vk::InstanceCreateInfo::default()
                .application_info(&app_info)
                .enabled_extension_names(&inst_exts);
            let instance = entry.create_instance(&inst_info, None)
                .expect("vkCreateInstance failed");

            // Win32 surface bound directly to our HWND.
            let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
            let win32_loader = ash::khr::win32_surface::Instance::new(&entry, &instance);
            let surface_info = vk::Win32SurfaceCreateInfoKHR::default()
                .hinstance(hinstance as _)
                .hwnd(hwnd as _);
            let surface = win32_loader
                .create_win32_surface(&surface_info, None)
                .expect("vkCreateWin32SurfaceKHR failed");

            // Pick the first physical device that has at least one queue
            // family supporting both graphics and present on this surface.
            let phys_devices = instance.enumerate_physical_devices().unwrap();
            let (physical_device, queue_family) = phys_devices.iter()
                .find_map(|&pd| {
                    let qprops = instance.get_physical_device_queue_family_properties(pd);
                    qprops.iter().enumerate().find_map(|(i, p)| {
                        let gfx = p.queue_flags.contains(vk::QueueFlags::GRAPHICS);
                        let present = surface_loader
                            .get_physical_device_surface_support(pd, i as u32, surface)
                            .unwrap_or(false);
                        if gfx && present { Some((pd, i as u32)) } else { None }
                    })
                })
                .expect("no Vulkan physical device with graphics + present");

            // Logical device with a single queue from that family and the
            // swapchain extension enabled.
            let priorities = [1.0f32];
            let qinfo = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&priorities)];
            let dev_exts = [ash::khr::swapchain::NAME.as_ptr()];
            let dev_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&qinfo)
                .enabled_extension_names(&dev_exts);
            let device = instance.create_device(physical_device, &dev_info, None)
                .expect("vkCreateDevice failed");
            let queue = device.get_device_queue(queue_family, 0);

            let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);

            let (swapchain, swapchain_format, swapchain_extent, swapchain_views) =
                Self::build_swapchain(
                    &surface_loader, &swapchain_loader, &device,
                    physical_device, surface, vk::SwapchainKHR::null());

            let render_pass = Self::build_render_pass(&device, swapchain_format);
            let framebuffers = Self::build_framebuffers(
                &device, render_pass, &swapchain_views, swapchain_extent);

            let pool_info = vk::CommandPoolCreateInfo::default()
                .queue_family_index(queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
            let command_pool = device.create_command_pool(&pool_info, None).unwrap();

            let cb_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(MAX_FRAMES_IN_FLIGHT as u32);
            let command_buffers = device.allocate_command_buffers(&cb_info).unwrap();

            // Per frame sync. Fences start signaled so the very first
            // wait_for_fences in render_frame returns immediately.
            let sem_info = vk::SemaphoreCreateInfo::default();
            let fence_info = vk::FenceCreateInfo::default()
                .flags(vk::FenceCreateFlags::SIGNALED);

            let mut image_available = [vk::Semaphore::null(); MAX_FRAMES_IN_FLIGHT];
            let mut render_finished = [vk::Semaphore::null(); MAX_FRAMES_IN_FLIGHT];
            let mut in_flight       = [vk::Fence::null();     MAX_FRAMES_IN_FLIGHT];
            for i in 0..MAX_FRAMES_IN_FLIGHT {
                image_available[i] = device.create_semaphore(&sem_info, None).unwrap();
                render_finished[i] = device.create_semaphore(&sem_info, None).unwrap();
                in_flight[i]       = device.create_fence(&fence_info, None).unwrap();
            }

            Renderer {
                _entry: entry,
                instance,
                surface_loader,
                surface,
                physical_device,
                queue_family,
                device,
                queue,
                swapchain_loader,
                swapchain,
                swapchain_format,
                swapchain_extent,
                swapchain_views,
                render_pass,
                framebuffers,
                command_pool,
                command_buffers,
                image_available,
                render_finished,
                in_flight,
                current_frame: 0,
            }
        }
    }

    /// Build a brand new swapchain (and matching image views) for the
    /// surface. If `old` is non-null it is supplied to the driver as a
    /// hint and must be destroyed by the caller after this returns.
    unsafe fn build_swapchain(
        surface_loader:   &ash::khr::surface::Instance,
        swapchain_loader: &ash::khr::swapchain::Device,
        device:           &Device,
        physical_device:  vk::PhysicalDevice,
        surface:          vk::SurfaceKHR,
        old:              vk::SwapchainKHR,
    ) -> (vk::SwapchainKHR, vk::Format, vk::Extent2D, Vec<vk::ImageView>) {
        let caps = surface_loader
            .get_physical_device_surface_capabilities(physical_device, surface).unwrap();
        let formats = surface_loader
            .get_physical_device_surface_formats(physical_device, surface).unwrap();
        let modes = surface_loader
            .get_physical_device_surface_present_modes(physical_device, surface).unwrap();

        // Prefer a plain BGRA8 sRGB-nonlinear swapchain (universally
        // available on desktop GPUs); fall back to whatever the driver
        // returns first.
        let format = formats.iter().copied()
            .find(|f| {
                f.format == vk::Format::B8G8R8A8_UNORM
                && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .unwrap_or(formats[0]);

        // MAILBOX gives lowest latency tearing-free presentation when
        // available; FIFO is the only mode guaranteed by the spec.
        let present_mode = if modes.contains(&vk::PresentModeKHR::MAILBOX) {
            vk::PresentModeKHR::MAILBOX
        } else {
            vk::PresentModeKHR::FIFO
        };

        // Some platforms report u32::MAX in current_extent meaning "you
        // pick". Choose a sane fallback in that case.
        let extent = if caps.current_extent.width == u32::MAX {
            vk::Extent2D { width: 1280, height: 720 }
        } else {
            caps.current_extent
        };

        // Try to keep one extra image to avoid stalls but stay inside the
        // driver provided maximum if any.
        let mut image_count = caps.min_image_count + 1;
        if caps.max_image_count > 0 && image_count > caps.max_image_count {
            image_count = caps.max_image_count;
        }

        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old);

        let swapchain = swapchain_loader.create_swapchain(&info, None).unwrap();
        let images = swapchain_loader.get_swapchain_images(swapchain).unwrap();

        let views: Vec<_> = images.iter().map(|&img| {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(img)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format.format)
                .components(vk::ComponentMapping::default())
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask:      vk::ImageAspectFlags::COLOR,
                    base_mip_level:   0, level_count: 1,
                    base_array_layer: 0, layer_count: 1,
                });
            device.create_image_view(&view_info, None).unwrap()
        }).collect();

        (swapchain, format.format, extent, views)
    }

    /// Single subpass render pass that clears the color attachment on
    /// load and stores it for presentation. All real geometry will live
    /// inside this same subpass for now.
    unsafe fn build_render_pass(device: &Device, format: vk::Format) -> vk::RenderPass {
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

        // External to subpass 0 dependency so the implicit layout
        // transition from PRESENT_SRC to COLOR_ATTACHMENT happens after
        // the swapchain image is acquired.
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

    /// One framebuffer per swapchain image view.
    unsafe fn build_framebuffers(
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
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            device.create_framebuffer(&info, None).unwrap()
        }).collect()
    }

    /// Tear down everything that depends on the surface size and rebuild
    /// it. Called after WM_SIZE or after a swapchain becomes out of date.
    pub fn recreate_swapchain(&mut self) {
        unsafe {
            self.device.device_wait_idle().ok();

            for &fb in &self.framebuffers { self.device.destroy_framebuffer(fb, None); }
            for &v  in &self.swapchain_views { self.device.destroy_image_view(v, None); }
            self.framebuffers.clear();
            self.swapchain_views.clear();

            let old = self.swapchain;
            let (sc, fmt, ext, views) = Self::build_swapchain(
                &self.surface_loader, &self.swapchain_loader, &self.device,
                self.physical_device, self.surface, old);
            self.swapchain_loader.destroy_swapchain(old, None);

            self.swapchain        = sc;
            self.swapchain_format = fmt;
            self.swapchain_extent = ext;
            self.swapchain_views  = views;

            // Render pass keeps the same format so it does not need to be
            // rebuilt. Framebuffers do, because they bake in the size.
            self.framebuffers = Self::build_framebuffers(
                &self.device, self.render_pass, &self.swapchain_views, self.swapchain_extent);
        }
    }

    /// Submit a single frame. The animated clear colors prove that the
    /// CPU side game loop, the GPU command stream and the Win32 message
    /// pump are all running together.
    pub fn render_frame(&mut self, time: f32) {
        unsafe {
            // Skip frames while the window is minimized to avoid
            // allocating zero-sized swapchains on resize.
            if self.swapchain_extent.width == 0 || self.swapchain_extent.height == 0 {
                return;
            }

            let frame = self.current_frame;
            self.device.wait_for_fences(&[self.in_flight[frame]], true, u64::MAX).unwrap();

            let acquire = self.swapchain_loader.acquire_next_image(
                self.swapchain, u64::MAX, self.image_available[frame], vk::Fence::null());
            let image_index = match acquire {
                Ok((idx, _)) => idx,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.recreate_swapchain();
                    return;
                }
                Err(e) => panic!("acquire_next_image: {:?}", e),
            };

            self.device.reset_fences(&[self.in_flight[frame]]).unwrap();

            let cmd = self.command_buffers[frame];
            self.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).unwrap();

            let begin = vk::CommandBufferBeginInfo::default();
            self.device.begin_command_buffer(cmd, &begin).unwrap();

            // Outer background pulse, slow.
            let bg = 0.04 + 0.04 * (time * 0.7).sin().abs();
            let clear_values = [vk::ClearValue {
                color: vk::ClearColorValue { float32: [bg, bg * 0.5, bg * 1.5, 1.0] },
            }];

            let rp_begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain_extent,
                })
                .clear_values(&clear_values);
            self.device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);

            // Centered square that represents the playfield. This is
            // computed every frame from the live extent so any aspect
            // ratio (4:3, 16:9, 21:9, 32:9, portrait, anything) just
            // works without code changes.
            let (sx, sy, ss) = playfield_rect(self.swapchain_extent.width,
                                              self.swapchain_extent.height);
            let pulse = 0.15 + 0.15 * (time * 1.7).sin().abs();
            let clear_attach = [vk::ClearAttachment {
                aspect_mask:      vk::ImageAspectFlags::COLOR,
                color_attachment: 0,
                clear_value:      vk::ClearValue { color: vk::ClearColorValue {
                    float32: [pulse, pulse * 0.3, pulse * 0.6, 1.0],
                }},
            }];
            let clear_rect = [vk::ClearRect {
                rect: vk::Rect2D {
                    offset: vk::Offset2D { x: sx as i32, y: sy as i32 },
                    extent: vk::Extent2D { width: ss, height: ss },
                },
                base_array_layer: 0,
                layer_count:      1,
            }];
            self.device.cmd_clear_attachments(cmd, &clear_attach, &clear_rect);

            // Future hook: bind a pipeline and draw the hexagon, walls,
            // player and HUD here using the same `cmd`.

            self.device.cmd_end_render_pass(cmd);
            self.device.end_command_buffer(cmd).unwrap();

            let wait_sems   = [self.image_available[frame]];
            let signal_sems = [self.render_finished[frame]];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let cmd_bufs    = [cmd];

            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&wait_sems)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&cmd_bufs)
                .signal_semaphores(&signal_sems);
            self.device.queue_submit(self.queue, &[submit], self.in_flight[frame]).unwrap();

            let swapchains = [self.swapchain];
            let indices    = [image_index];
            let present = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_sems)
                .swapchains(&swapchains)
                .image_indices(&indices);
            match self.swapchain_loader.queue_present(self.queue, &present) {
                Ok(_) => {}
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR)
                | Err(vk::Result::SUBOPTIMAL_KHR) => {
                    self.recreate_swapchain();
                }
                Err(e) => panic!("queue_present: {:?}", e),
            }

            self.current_frame = (self.current_frame + 1) % MAX_FRAMES_IN_FLIGHT;
        }
    }

    /// Release every Vulkan object in reverse order of creation.
    pub fn destroy(&mut self) {
        unsafe {
            self.device.device_wait_idle().ok();

            for i in 0..MAX_FRAMES_IN_FLIGHT {
                self.device.destroy_semaphore(self.image_available[i], None);
                self.device.destroy_semaphore(self.render_finished[i], None);
                self.device.destroy_fence(self.in_flight[i], None);
            }
            self.device.destroy_command_pool(self.command_pool, None);

            for &fb in &self.framebuffers { self.device.destroy_framebuffer(fb, None); }
            self.device.destroy_render_pass(self.render_pass, None);
            for &v in &self.swapchain_views { self.device.destroy_image_view(v, None); }
            self.swapchain_loader.destroy_swapchain(self.swapchain, None);

            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Compute the centered square sub-region of a `w x h` framebuffer that
/// will host the actual game. The remaining pixels become letterbox or
/// pillarbox depending on the aspect ratio.
///
/// Returns `(x, y, side)` where `(x, y)` is the top-left corner inside
/// the framebuffer and `side` is the length of the square in pixels.
pub fn playfield_rect(w: u32, h: u32) -> (u32, u32, u32) {
    let s = w.min(h);
    let x = (w - s) / 2;
    let y = (h - s) / 2;
    (x, y, s)
}