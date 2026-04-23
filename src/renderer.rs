//! Vulkan renderer.
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
//! * Own a single graphics pipeline (`crate::pipeline`) plus one host
//!   visible vertex buffer per frame in flight (`crate::buffer`).
//! * Issue one render pass per frame that clears the full surface to
//!   the letterbox color and then draws the per frame vertex list
//!   inside a centered square viewport, so the playfield stays correct
//!   on any window aspect ratio.
//! * Issue one render pass per frame that clears the full surface
//!   to black (no longer used as letterbox), sets the viewport to
//!   the full framebuffer and pushes a `vec2` aspect scale to the
//!   vertex shader. The shader then squeezes the wider axis so a
//!   unit circle in game space stays round on any monitor (4:3,
//!   16:9, 21:9, 32:9, portrait, anything), without ever stretching
//!   the image. Wider monitors simply reveal more of the radial
//!   hex tunnel.
//! Notably absent on purpose: any descriptor sets, push constants,
//! audio, font rendering, or game logic. Adding them later does not
//! require restructuring anything in this file beyond `render_frame`.
//!
//! # VSync
//!
//! The present mode used when building the swapchain is now taken
//! from [`VsyncMode`]:
//!
//! * `Off`  -> `IMMEDIATE` if available, otherwise `MAILBOX`,
//!             otherwise `FIFO`.
//! * `On`   -> `FIFO` (guaranteed by the spec, no tearing).
//! * `Fast` -> `MAILBOX` if available, otherwise `FIFO`.
//!
//! # FPS cap
//!
//! The cap itself is driven by the main loop using a monotonic
//! clock; the renderer exposes a small helper, [`FramePacer`],
//! that encapsulates "sleep until target_time" without drifting.
//!
//! # Post-process pipeline
//!
//! As of this revision the renderer no longer draws the scene
//! directly into the swapchain. Instead [`crate::post::PostStage`]
//! owns an offscreen color target; the main pipeline renders the
//! scene into it, then a fullscreen post pipeline samples the
//! offscreen target and writes the final image into the swapchain,
//! applying bloom / vignette / chromatic aberration / scanlines /
//! film grain / colorblind / high-contrast effects driven by
//! [`crate::post::PostParams`]. The effect parameters are pushed
//! once per frame through [`Renderer::set_post_params`].

use ash::{vk, Device, Entry, Instance};
use std::ffi::{c_void, CString};
use std::time::{Duration, Instant};

use crate::buffer::DynamicVertexBuffer;
use crate::config::VsyncMode;
use crate::pipeline::{Pipeline, PushConstants, Vertex};
use crate::post::{PostParams, PostStage};

/// How many frames may be in flight on the GPU simultaneously. Two is a
/// good compromise between latency and CPU/GPU overlap for a fast paced
/// game like this one.
const MAX_FRAMES_IN_FLIGHT: usize = 2;

/// Maximum number of vertices a single frame may submit. Used to size
/// every per frame vertex buffer at startup. A few thousand is plenty
/// for the current art style; pick a power of two for nice alignment.
const VERTEX_CAPACITY: usize = 131_072;

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

    /// Post-process stage. Owns the offscreen target, both
    /// render passes (offscreen + post), the swapchain
    /// framebuffers and the post pipeline / descriptor set.
    /// The main pipeline is created against
    /// `post.offscreen_render_pass()`.
    post: PostStage,

    command_pool:     vk::CommandPool,
    command_buffers:  Vec<vk::CommandBuffer>,

    /// Single graphics pipeline used for every draw call. Viewport and
    /// scissor are dynamic, so this object survives window resizes.
    pipeline:         Pipeline,
    /// One host visible vertex buffer per frame in flight, written by
    /// the CPU each frame and read directly by the GPU.
    vertex_buffers:   Vec<DynamicVertexBuffer>,

    image_available:  [vk::Semaphore; MAX_FRAMES_IN_FLIGHT],
    render_finished:  [vk::Semaphore; MAX_FRAMES_IN_FLIGHT],
    in_flight:        [vk::Fence;     MAX_FRAMES_IN_FLIGHT],
    current_frame:    usize,

    vsync:            VsyncMode,
    vsync_dirty:      bool,

    shake_offset:     [f32; 2],
    post_params:      PostParams,
    zoom_scale:       f32,
}

impl Renderer {
    /// Bring up Vulkan and produce a fully usable renderer bound to the
    /// supplied Win32 window. Panics on any unrecoverable setup failure;
    /// for a skeleton this is acceptable, real shipping code would
    /// surface these errors to the user.
    pub fn new(hinstance: *mut c_void, hwnd: *mut c_void, vsync: VsyncMode) -> Self {
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
                    physical_device, surface, vk::SwapchainKHR::null(), vsync);

            // Post stage owns both render passes and the offscreen
            // target. The main pipeline below is built against its
            // offscreen render pass.
            let post = PostStage::new(
                &instance, physical_device, &device,
                swapchain_format, &swapchain_views, swapchain_extent);

            let pool_info = vk::CommandPoolCreateInfo::default()
                .queue_family_index(queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
            let command_pool = device.create_command_pool(&pool_info, None).unwrap();

            let cb_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(MAX_FRAMES_IN_FLIGHT as u32);
            let command_buffers = device.allocate_command_buffers(&cb_info).unwrap();

            // Pipeline only depends on the render pass being compatible,
            // not on its concrete dimensions, so it lives across resizes.
            // We wire it to the offscreen render pass owned by PostStage.
            let pipeline = Pipeline::new(&device, post.offscreen_render_pass());

            // One persistently mapped vertex buffer per frame in flight,
            // so the CPU can write frame N+1 while the GPU consumes
            // frame N without any extra synchronization.
            let mut vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
            for _ in 0..MAX_FRAMES_IN_FLIGHT {
                vertex_buffers.push(DynamicVertexBuffer::new(
                    &instance, physical_device, &device, VERTEX_CAPACITY));
            }

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
                post,
                command_pool,
                command_buffers,
                pipeline,
                vertex_buffers,
                image_available,
                render_finished,
                in_flight,
                current_frame: 0,
                vsync,
                vsync_dirty: false,
                shake_offset: [0.0, 0.0],
                zoom_scale:   1.0,
                post_params: PostParams::default(),
            }
        }
    }

    /// Change the vsync mode. Takes effect on the next frame by
    /// forcing a swapchain rebuild.
    pub fn set_vsync(&mut self, v: VsyncMode) {
        if self.vsync != v {
            self.vsync = v;
            self.vsync_dirty = true;
        }
    }

    pub fn vsync(&self) -> VsyncMode { self.vsync }

    /// Set per-frame screen-shake offset, in game coordinates. The
    /// value persists until changed so the caller does not need to
    /// reset it every frame when unused.
    pub fn set_shake(&mut self, offset: [f32; 2]) {
        self.shake_offset = offset;
    }

    /// Publish the parameters the post fragment shader should use
    /// for the next frame. Stored as-is and uploaded via push
    /// constants inside `render_frame`.
    pub fn set_post_params(&mut self, params: PostParams) {
        self.post_params = params;
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
        vsync:            VsyncMode,
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

        // Choose present mode per requested vsync policy. Every
        // fallback chain ends in FIFO, which the Vulkan spec
        // guarantees is always supported.
        let present_mode = match vsync {
            VsyncMode::Off => {
                if modes.contains(&vk::PresentModeKHR::IMMEDIATE) {
                    vk::PresentModeKHR::IMMEDIATE
                } else if modes.contains(&vk::PresentModeKHR::MAILBOX) {
                    vk::PresentModeKHR::MAILBOX
                } else {
                    vk::PresentModeKHR::FIFO
                }
            }
            VsyncMode::On   => vk::PresentModeKHR::FIFO,
            VsyncMode::Fast => {
                if modes.contains(&vk::PresentModeKHR::MAILBOX) {
                    vk::PresentModeKHR::MAILBOX
                } else {
                    vk::PresentModeKHR::FIFO
                }
            }
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

    /// Tear down everything that depends on the surface size and rebuild
    /// it. Called after WM_SIZE or after a swapchain becomes out of date.
    pub fn recreate_swapchain(&mut self) {
        unsafe {
            self.device.device_wait_idle().ok();

            for &v in &self.swapchain_views { self.device.destroy_image_view(v, None); }
            self.swapchain_views.clear();

            let old = self.swapchain;
            let (sc, fmt, ext, views) = Self::build_swapchain(
                &self.surface_loader, &self.swapchain_loader, &self.device,
                self.physical_device, self.surface, old, self.vsync);
            self.swapchain_loader.destroy_swapchain(old, None);

            self.swapchain        = sc;
            self.swapchain_format = fmt;
            self.swapchain_extent = ext;
            self.swapchain_views  = views;

            // PostStage owns the offscreen image and all framebuffers
            // (both offscreen and swapchain); ask it to rebuild them
            // at the new size and rebind the descriptor set.
            self.post.recreate(
                &self.instance, self.physical_device, &self.device,
                &self.swapchain_views, self.swapchain_extent);
        }
        self.vsync_dirty = false;
    }

    /// Upload the supplied vertex list into this frame's vertex buffer
    /// and submit one draw call covering the whole list.
    ///
    /// The vertex positions are expected to already be in Vulkan NDC,
    /// produced under the assumption that the viewport covers a
    /// centered square. The renderer enforces that assumption here by
    /// programming a dynamic viewport plus scissor matching exactly
    /// that square; everything outside is filled with the render pass
    /// clear color (black) and serves as letterbox or pillarbox area.
    pub fn render_frame(&mut self, vertices: &[Vertex]) {
        if self.vsync_dirty {
            self.recreate_swapchain();
        }

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

            // Safe to write because the fence above guarantees the GPU
            // is done reading the previous contents of this same buffer.
            self.vertex_buffers[frame].upload(vertices);
            let vertex_count = self.vertex_buffers[frame].count as u32;

            let cmd = self.command_buffers[frame];
            self.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()).unwrap();

            let begin = vk::CommandBufferBeginInfo::default();
            self.device.begin_command_buffer(cmd, &begin).unwrap();

            // ---- pass 1: main scene into the offscreen target ----

            self.post.begin_offscreen(&self.device, cmd, self.swapchain_extent);

            // Full screen viewport plus matching scissor. We no
            // longer carve out a centered square: the pipeline's
            // vertex shader applies a `vec2 scale` push constant
            // that takes care of aspect correction without losing
            // any pixels to letterboxing.
            let viewport = vk::Viewport {
                x:         0.0,
                y:         0.0,
                width:     self.swapchain_extent.width  as f32,
                height:    self.swapchain_extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain_extent,
            };
            self.device.cmd_set_viewport(cmd, 0, &[viewport]);
            self.device.cmd_set_scissor (cmd, 0, &[scissor]);

            // Push the aspect scale + shake offset to the vertex
            // shader. 16 bytes total; sent as a raw byte slice.
            let (sx, sy) = aspect_scale(self.swapchain_extent.width,
                                        self.swapchain_extent.height);
            let pc = PushConstants {
                scale: [sx, sy],
                shake: self.shake_offset,
                zoom:  self.zoom_scale,
                _pad:  [0.0, 0.0, 0.0],
            };
            let push_bytes = std::slice::from_raw_parts(
                (&pc as *const PushConstants) as *const u8,
                std::mem::size_of::<PushConstants>(),
            );
            self.device.cmd_push_constants(
                cmd,
                self.pipeline.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                push_bytes,
            );

            // Bind once, draw the entire triangle list. With no index
            // buffer and no instancing this is the minimum amount of
            // command stream needed to put pixels on screen.
            if vertex_count > 0 {
                self.device.cmd_bind_pipeline(
                    cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline.pipeline);
                self.device.cmd_bind_vertex_buffers(
                    cmd, 0, &[self.vertex_buffers[frame].buffer], &[0]);
                self.device.cmd_draw(cmd, vertex_count, 1, 0, 0);
            }

            self.post.end_render_pass(&self.device, cmd);

            // ---- pass 2: post process into the swapchain ----

            // The resolution field is filled per frame because the
            // window can resize between calls.
            let mut params = self.post_params;
            params.resolution = [
                self.swapchain_extent.width  as f32,
                self.swapchain_extent.height as f32,
            ];
            self.post.render_post(
                &self.device, cmd, image_index,
                self.swapchain_extent, &params);

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

            // Per frame vertex buffers and the shared pipeline both
            // outlive a single frame, so they live here at the top of
            // the teardown sequence, just below the device wait.
            for vb in &self.vertex_buffers {
                vb.destroy(&self.device);
            }
            self.pipeline.destroy(&self.device);

            for i in 0..MAX_FRAMES_IN_FLIGHT {
                self.device.destroy_semaphore(self.image_available[i], None);
                self.device.destroy_semaphore(self.render_finished[i], None);
                self.device.destroy_fence(self.in_flight[i], None);
            }
            self.device.destroy_command_pool(self.command_pool, None);

            // PostStage owns both render passes and all framebuffers.
            // Destroy it before the swapchain views it references.
            self.post.destroy(&self.device);

            for &v in &self.swapchain_views { self.device.destroy_image_view(v, None); }
            self.swapchain_loader.destroy_swapchain(self.swapchain, None);

            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }

    /// Uniform camera zoom applied in the vertex shader before
    /// aspect correction. Values above 1.0 zoom in.
    pub fn set_zoom(&mut self, z: f32) {
        self.zoom_scale = z.clamp(0.25, 3.0);
    }
}

/// Compute the per axis scale the vertex shader applies to keep a
/// unit circle round on any framebuffer aspect ratio.
///
/// Convention: the longer axis ends up at scale 1.0 (that is, game
/// space coordinates `[-1, 1]` map directly onto NDC `[-1, 1]` along
/// the long axis). The shorter axis is squeezed by the inverse
/// aspect ratio, so a square drawn at `[-1, 1]` in game space ends
/// up visually square instead of stretched.
///
/// The same function is used by the menu module to convert mouse
/// pixel positions back into game space, so widget hit boxes and
/// rendered widget shapes stay perfectly aligned regardless of the
/// window aspect ratio.
pub fn aspect_scale(w: u32, h: u32) -> (f32, f32) {
    let w = w.max(1) as f32;
    let h = h.max(1) as f32;
    if w >= h {
        (h / w, 1.0)
    } else {
        (1.0, w / h)
    }
}

/// Dead simple frame pacer. The main loop keeps a `FramePacer`
/// instance and calls [`FramePacer::begin`] / [`FramePacer::wait`]
/// around each iteration. At zero target FPS the pacer is a no-op
/// so uncapped frame rates do not pay for unused logic.
///
/// The implementation combines a coarse sleep for the bulk of the
/// wait and a tight spin for the last ~1 ms, which is accurate on
/// Windows without relying on `timeBeginPeriod`.
pub struct FramePacer {
    target: Option<Duration>,
    next_frame: Instant,
}

impl FramePacer {
    pub fn new(fps: u32) -> Self {
        FramePacer {
            target: if fps == 0 { None } else { Some(Duration::from_secs_f64(1.0 / fps as f64)) },
            next_frame: Instant::now(),
        }
    }

    pub fn set_fps(&mut self, fps: u32) {
        self.target = if fps == 0 {
            None
        } else {
            Some(Duration::from_secs_f64(1.0 / fps as f64))
        };
        self.next_frame = Instant::now();
    }

    pub fn begin(&mut self) {
        if self.target.is_none() { return; }
        let now = Instant::now();
        if now > self.next_frame + Duration::from_millis(100) {
            self.next_frame = now;
        }
    }

    /// Block until the next frame slot opens. Uses a hybrid sleep +
    /// spin strategy to stay within ~0.5 ms of the target.
    pub fn wait(&mut self) {
        let Some(period) = self.target else { return; };
        self.next_frame += period;
        let now = Instant::now();
        if self.next_frame <= now { return; }
        let remaining = self.next_frame - now;
        if remaining > Duration::from_millis(2) {
            std::thread::sleep(remaining - Duration::from_millis(1));
        }
        while Instant::now() < self.next_frame {
            std::hint::spin_loop();
        }
    }
}