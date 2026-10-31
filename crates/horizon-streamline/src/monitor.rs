//! Optional Vulkan timestamps. Readback never waits for the current frame.
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
const SLOTS: usize = 8;
const STRIDE: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}
pub(crate) struct Monitor {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    next: usize,
    period: f64,
    pub gpu_ms: Option<[f32; 2]>,
    pub samples: u64,
}
impl Monitor {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        let flags =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        if !device.features().contains(flags) {
            return None;
        }
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("DLSS stage timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: 4 * SLOTS as u32,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("DLSS timestamp resolve"),
                size: STRIDE * SLOTS as u64,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            slots: (0..SLOTS)
                .map(|_| Slot {
                    buffer: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("DLSS timing readback"),
                        size: 32,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }),
                    state: Arc::new(AtomicU8::new(0)),
                })
                .collect(),
            next: 0,
            period: f64::from(queue.get_timestamp_period()),
            gpu_ms: None,
            samples: 0,
        })
    }
    pub fn begin(&mut self, device: &wgpu::Device) -> Option<usize> {
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &self.slots {
            if slot.state.load(Ordering::Acquire) != 2 {
                continue;
            }
            if let Ok(data) = slot.buffer.slice(..).get_mapped_range() {
                let t: [u64; 4] = std::array::from_fn(|i| {
                    u64::from_le_bytes(data[i * 8..i * 8 + 8].try_into().unwrap())
                });
                if let (Some(copy), Some(dlss)) = (t[1].checked_sub(t[0]), t[3].checked_sub(t[2])) {
                    let sample = [copy, dlss].map(|v| (v as f64 * self.period / 1e6) as f32);
                    self.gpu_ms = Some(self.gpu_ms.map_or(sample, |old| {
                        std::array::from_fn(|i| old[i] * 0.9 + sample[i] * 0.1)
                    }));
                    self.samples += 1;
                }
            }
            slot.buffer.unmap();
            slot.state.store(0, Ordering::Release);
        }
        for _ in 0..SLOTS {
            let at = self.next;
            self.next = (at + 1) % SLOTS;
            if self.slots[at]
                .state
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(at);
            }
        }
        None
    }
    pub fn stamp(&self, encoder: &mut wgpu::CommandEncoder, slot: usize, point: u32) {
        encoder.write_timestamp(&self.queries, slot as u32 * 4 + point);
    }
    pub fn end(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        self.stamp(encoder, slot, 3);
        let first = slot as u32 * 4;
        let offset = slot as u64 * STRIDE;
        encoder.resolve_query_set(&self.queries, first..first + 4, &self.resolve, offset);
        encoder.copy_buffer_to_buffer(&self.resolve, offset, &self.slots[slot].buffer, 0, 32);
    }
    pub fn submitted(&self, slot: usize) {
        let state = self.slots[slot].state.clone();
        self.slots[slot]
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { 2 } else { 0 }, Ordering::Release);
            });
    }
    pub fn cancel(&self, slot: usize) {
        self.slots[slot].state.store(0, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Performance {
    pub cpu_total_ms: f32,
    pub cpu_copy_ms: f32,
    pub cpu_evaluate_ms: f32,
    /// Input conversion/copies, then native DLSS plus its transitions.
    pub gpu_ms: Option<[f32; 2]>,
    pub gpu_samples: u64,
    pub completed_frames: u64,
    pub reset_frames: u64,
}
