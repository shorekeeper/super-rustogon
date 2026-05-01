//! Persistently mapped, host visible vertex buffer.
//!
//! One instance is created per frame in flight, so the CPU can rewrite
//! the geometry for frame N+1 while the GPU is still consuming frame N
//! without any synchronization beyond the per frame fence the renderer
//! already uses to gate command buffer reuse.
//!
//! Memory properties:
//!
//! * `HOST_VISIBLE` lets the CPU write to it directly.
//! * `HOST_COHERENT` removes the need for explicit
//!   `vkFlushMappedMemoryRanges` calls; a plain `memcpy` into the
//!   mapped pointer is enough.
//!
//! Capacity is fixed at construction. For a Super Hexagon style game a
//! few thousand vertices per frame is more than enough; the default
//! used by the renderer is 16k vertices.

use ash::{vk, Device, Instance};
use std::ptr;

use crate::pipeline::Vertex;

/// Single CPU-writable vertex buffer with its backing memory mapped for
/// the entire lifetime of the object.
pub struct DynamicVertexBuffer {
    pub buffer:   vk::Buffer,
    pub memory:   vk::DeviceMemory,
    /// Maximum number of `Vertex` elements the buffer can hold.
    pub capacity: usize,
    /// Number of vertices written by the most recent `upload`. The
    /// renderer reads this to know how many to draw.
    pub count:    usize,
    /// Host pointer to the start of the buffer. Persistently mapped.
    mapped:       *mut Vertex,
}

impl DynamicVertexBuffer {
    /// Allocate a buffer of `capacity` vertices and map it permanently.
    pub fn new(
        instance:        &Instance,
        physical_device: vk::PhysicalDevice,
        device:          &Device,
        capacity:        usize,
    ) -> Self {
        unsafe {
            let bytes = (capacity * std::mem::size_of::<Vertex>()) as vk::DeviceSize;

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
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            );
            let alloc = vk::MemoryAllocateInfo::default()
                .allocation_size(req.size)
                .memory_type_index(idx);
            let memory = device.allocate_memory(&alloc, None).unwrap();
            device.bind_buffer_memory(buffer, memory, 0).unwrap();

            let ptr_v = device
                .map_memory(memory, 0, bytes, vk::MemoryMapFlags::empty())
                .unwrap();

            DynamicVertexBuffer {
                buffer,
                memory,
                capacity,
                count:  0,
                mapped: ptr_v as *mut Vertex,
            }
        }
    }

    /// Copy a vertex slice into the mapped region, clamping to capacity.
    ///
    /// The caller is responsible for ensuring the GPU is no longer
    /// reading from this buffer; in the renderer that is guaranteed by
    /// waiting on the per frame in flight fence first.
    pub fn upload(&mut self, vertices: &[Vertex]) {
        let n = vertices.len().min(self.capacity);
        unsafe {
            ptr::copy_nonoverlapping(vertices.as_ptr(), self.mapped, n);
        }
        self.count = n;
    }

    /// Copy two vertex slices into the mapped region back to back.
    /// Used by the renderer to upload game and HUD geometry in one
    /// buffer so they can be drawn with different push constants
    /// through two `cmd_draw` calls without rebinding the buffer.
    pub fn upload_two(&mut self, first: &[Vertex], second: &[Vertex]) {
        let a_n = first.len().min(self.capacity);
        let b_n = second.len().min(self.capacity - a_n);
        unsafe {
            ptr::copy_nonoverlapping(first.as_ptr(), self.mapped, a_n);
            if b_n > 0 {
                ptr::copy_nonoverlapping(
                    second.as_ptr(), self.mapped.add(a_n), b_n);
            }
        }
        self.count = a_n + b_n;
    }

    /// Unmap, free the backing memory and destroy the buffer. Caller
    /// must ensure the device is idle.
    pub fn destroy(&self, device: &Device) {
        unsafe {
            device.unmap_memory(self.memory);
            device.destroy_buffer(self.buffer, None);
            device.free_memory(self.memory, None);
        }
    }
}

/// Find the index of a memory type that both satisfies the buffer's
/// `memoryTypeBits` mask and exposes the requested property flags.
/// Panics if no such memory type exists, which on a sane desktop GPU
/// driver should never happen for HOST_VISIBLE | HOST_COHERENT.
fn find_memory_type(
    props:     &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    flags:     vk::MemoryPropertyFlags,
) -> u32 {
    for i in 0..props.memory_type_count {
        let suitable = (type_bits & (1 << i)) != 0
            && props.memory_types[i as usize].property_flags.contains(flags);
        if suitable {
            return i;
        }
    }
    panic!("No memory type satisfying {:?}", flags);
}