//! Fixed-camera rasterization through the entity registration used by Game::mark_motion.
use super::{
    Vertex,
    motion::Motion,
    motion_history::{Source, Span},
};
use glam::{DVec3, Mat4};
use wgpu::util::DeviceExt;

#[test]
#[ignore = "requires Vulkan GPU; reads registered object motion pixels"]
fn object_motion_vulkan() {
    verify(wgpu::Backends::VULKAN);
}

#[cfg(windows)]
#[test]
#[ignore = "requires DX12 GPU; reads registered object motion pixels"]
fn object_motion_dx12() {
    verify(wgpu::Backends::DX12);
}

fn verify(backend: wgpu::Backends) {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backend;
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!("object motion readback: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let texture = |width, height, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("object motion regression"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let atlas = texture(
        2,
        1,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    queue.write_texture(
        atlas.as_image_copy(),
        &[255, 255, 255, 255, 255, 255, 255, 0],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 2,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let depth = texture(
        64,
        32,
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let depth = depth.create_view(&Default::default());
    let mut motion = Motion::new(&device, &atlas.create_view(&Default::default()));
    let indices: Vec<u32> = (0..6)
        .flat_map(|i| [0, 1, 2, 0, 2, 3].map(|j| i * 4 + j))
        .collect();
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 32,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for frame in 0..2 {
        let mut entities = Vec::new();
        let mut spans = Vec::new();
        // Hull/torso-shaped opaque quads stand in for boat/player meshes.
        // Every builder has ID 7: renderer namespaces must remain independent.
        for (source, x, y, z, uv, alpha, backwards) in [
            (Source::Boat, -0.9, 0.5, 0.5, 0.25, 255, false),
            (Source::Player, -0.2, 0.5, 0.5, 0.25, 255, false),
            (Source::Cart, 0.45, 0.5, 0.25, 0.25, 255, false), // behind opaque depth
            (Source::ArmorStand, -0.9, -0.3, 0.5, 0.25, 0, false), // vertex alpha
            (Source::Projectile, -0.2, -0.3, 0.5, 0.75, 255, false), // texture alpha
            (Source::Display, 0.45, -0.3, 0.5, 0.25, 255, true), // culled backface
        ] {
            let start = entities.len();
            let x = x + frame as f32 * 0.125;
            let positions = [
                [x, y, z],
                [x + 0.4, y, z],
                [x + 0.4, y + 0.4, z],
                [x, y + 0.4, z],
            ];
            for i in if backwards {
                [3, 2, 1, 0]
            } else {
                [0, 1, 2, 3]
            } {
                entities.push(Vertex {
                    pos: positions[i],
                    uv: [uv, 0.5],
                    color: (alpha as u32) << 24 | 0xffffff,
                    normal: [0, 0, 0, 1],
                    ..bytemuck::Zeroable::zeroed()
                });
            }
            Span::entity(&mut spans, source, 7, Some([1; 4]), start, entities.len());
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        // Reverse-Z scene: an opaque surface at 0.5, including behind cutouts.
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.5),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        }));
        assert!(motion.render(
            &device,
            &queue,
            &mut encoder,
            DVec3::ZERO,
            &spans,
            &entities,
            Mat4::IDENTITY,
            [0.0; 4],
            &depth,
            [64, 32],
            &indices
        ));
        let (target, _) = motion.tagged().unwrap();
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 64,
                height: 32,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        read.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        {
            let pixels = read.slice(..).get_mapped_range().unwrap();
            for (x, y, visible) in [
                (8, 5, true),
                (32, 5, true),
                (52, 5, false),
                (8, 17, false),
                (32, 17, false),
                (52, 17, false),
                (62, 30, false),
            ] {
                let offset = (y * 256 + x * 4) as usize;
                let value = |i| {
                    let h = u16::from_le_bytes([pixels[offset + i], pixels[offset + i + 1]]);
                    if h & 0x7fff == 0 {
                        0.0
                    } else {
                        let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
                        sign * (1.0 + f32::from(h & 1023) / 1024.0)
                            * 2.0f32.powi(i32::from((h >> 10) & 31) - 15)
                    }
                };
                let expected = if frame == 1 && visible { -0.0625 } else { 0.0 };
                assert!(
                    (value(0) - expected).abs() < 0.0001,
                    "{backend:?} frame {frame} pixel {x},{y}: {} expected {expected}",
                    value(0)
                );
                assert_eq!(value(2), 0.0);
            }
        }
        read.unmap();
    }
    assert!(pollster::block_on(scope.pop()).is_none());
}
