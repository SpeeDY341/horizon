//! Read actual world depth and production motion output, independently of SDK/FG availability.
use super::{motion::Motion, oit, world_pass, world_post::Post};
use glam::{DVec3, Mat4, Vec4};

#[test]
#[ignore = "requires a Vulkan GPU; reads rendered depth and motion"]
fn temporal_inputs_vulkan() {
    verify(wgpu::Backends::VULKAN);
}

#[cfg(windows)]
#[test]
#[ignore = "requires a DX12 GPU; reads rendered depth and motion"]
fn temporal_inputs_dx12() {
    verify(wgpu::Backends::DX12);
}

fn verify(backend: wgpu::Backends) {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backend;
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!("temporal input readback: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let texture = |format, size, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("temporal input regression"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
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
        wgpu::TextureFormat::Rgba8Unorm,
        1,
        wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let screen = texture(
        wgpu::TextureFormat::Rgba8Unorm,
        32,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let screen = screen.create_view(&Default::default());
    let hand_depth = texture(
        wgpu::TextureFormat::Depth32Float,
        32,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let hand_depth = hand_depth.create_view(&Default::default());
    let geometry = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            r#"
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                let j = i % 3u;
                let p = vec2(f32((j << 1u) & 2u), f32(j & 2u)) * 2.0 - 1.0;
                return vec4(p, select(0.25, 0.75, i >= 3u), 1.0);
            }
            @fragment fn fs() -> @location(0) vec4<f32> { return vec4(0.4, 0.5, 0.6, 1.0); }
        "#
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &geometry,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        multisample: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        fragment: Some(wgpu::FragmentState {
            module: &geometry,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    // Convert sampled RG16Float motion to f32 on the GPU, without a host half decoder.
    let sample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            r#"
            @group(0) @binding(0) var depth: texture_depth_2d;
            @group(0) @binding(1) var motion: texture_2d<f32>;
            @group(0) @binding(2) var<storage, read_write> output: array<vec4<f32>>;
            @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                let p = vec2<i32>(4 + i32(id.x) * 24, 16);
                output[id.x] = vec4(textureLoad(depth, p, 0), textureLoad(motion, p, 0).xy, 0.0);
            }
        "#
            .into(),
        ),
    });
    let sample = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &sample_shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let sampled = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 32,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 32,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let indices = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 24,
        usage: wgpu::BufferUsages::INDEX,
        mapped_at_creation: false,
    });
    let projection = super::world_projection(90.0, 1.0, 512.0);
    for aa in [0, 1, 2] {
        // Off, FXAA, TAA; FG requested in each case.
        for culling in [false, true] {
            for oit_on in [false, true] {
                let mut post = Post::new(&device, wgpu::TextureFormat::Rgba8Unorm);
                post.antialias = aa == 1;
                post.taa = aa == 2;
                post.track_camera = true; // Production FG-without-TAA camera path.
                let mut motion = Motion::new(&device, &atlas.create_view(&Default::default()));
                for frame in 0..2 {
                    let camera = DVec3::new(f64::from(frame) * 0.1, 0.0, 0.0);
                    post.jitter([32, 32], true, projection, camera);
                    let (color, depth) = post.prepare(&device, [32, 32], true).unwrap();
                    let jitter = post.temporal_jitter().unwrap();
                    let keep = post.keep_depth(false, oit_on, true);
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = world_pass(
                            &mut encoder,
                            &color,
                            &depth,
                            Some(wgpu::Color::BLACK),
                            true,
                            culling || keep,
                        );
                        pass.set_pipeline(&pipeline);
                        pass.set_scissor_rect(0, 0, 16, 32);
                        pass.draw(0..3, 0..1);
                        if !culling {
                            pass.set_scissor_rect(16, 0, 16, 32);
                            pass.draw(3..6, 0..1);
                        }
                    }
                    if culling {
                        // Same final pass used by both flat and indirect culling paths.
                        let mut pass = world_pass(&mut encoder, &color, &depth, None, false, keep);
                        pass.set_pipeline(&pipeline);
                        pass.set_scissor_rect(16, 0, 16, 32);
                        pass.draw(3..6, 0..1);
                    }
                    if oit_on {
                        // Exercise the production final OIT attachment operations.
                        drop(oit::composite_pass(&mut encoder, &color, &depth));
                    }
                    assert!(motion.render(
                        &device,
                        &queue,
                        &mut encoder,
                        camera,
                        &[],
                        &[],
                        projection,
                        jitter,
                        &depth,
                        [32, 32],
                        &indices
                    ));
                    post.finish(&queue, &mut encoder, &screen);
                    // Production hand/HUD clear the screen depth after motion; world depth must survive.
                    drop(world_pass(
                        &mut encoder,
                        &screen,
                        &hand_depth,
                        None,
                        true,
                        false,
                    ));
                    let (tag_depth, tag_view) = post.depth_inputs().unwrap();
                    let (tag_motion, motion_view) = motion.tagged().unwrap();
                    assert_eq!(tag_depth.format(), wgpu::TextureFormat::Depth32Float);
                    assert_eq!(tag_motion.format(), wgpu::TextureFormat::Rg16Float);
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &sample.get_bind_group_layout(0),
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&tag_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&motion_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: sampled.as_entire_binding(),
                            },
                        ],
                    });
                    {
                        let mut pass = encoder.begin_compute_pass(&Default::default());
                        pass.set_pipeline(&sample);
                        pass.set_bind_group(0, &group, &[]);
                        pass.dispatch_workgroups(2, 1, 1);
                    }
                    encoder.copy_buffer_to_buffer(&sampled, 0, &read, 0, 32);
                    queue.submit([encoder.finish()]);
                    read.slice(..)
                        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
                    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                    {
                        let bytes = read.slice(..).get_mapped_range().unwrap();
                        let values: &[f32] = bytemuck::cast_slice(&bytes);
                        for (i, z) in [0.25, 0.75].into_iter().enumerate() {
                            let actual = &values[i * 4..i * 4 + 3];
                            assert_eq!(
                                actual[0], z,
                                "depth: AA {aa}, culling {culling}, OIT {oit_on}"
                            );
                            let ndc = Vec4::new(
                                (4.5 + i as f32 * 24.0) / 16.0 - 1.0 - jitter[0],
                                1.0 - 16.5 / 16.0 - jitter[1],
                                z,
                                1.0,
                            );
                            let previous = projection
                                * Mat4::from_translation(glam::Vec3::new(0.1, 0.0, 0.0))
                                * projection.inverse()
                                * ndc;
                            let expected = if frame == 0 {
                                0.0
                            } else {
                                (previous.x / previous.w - ndc.x) * 0.5
                            };
                            assert!(
                                (actual[1] - expected).abs() < 0.002,
                                "motion: {actual:?}, expected {expected}, AA {aa}, culling {culling}, OIT {oit_on}"
                            );
                            assert!(actual[2].abs() < 0.001);
                        }
                    }
                    read.unmap();
                    // Bad size/projection must not make a previous target a valid current input.
                    if frame == 1 {
                        for (matrix, size) in [(projection, [31, 32]), (Mat4::ZERO, [32, 32])] {
                            assert!(!motion.render(
                                &device,
                                &queue,
                                &mut device.create_command_encoder(&Default::default()),
                                camera,
                                &[],
                                &[],
                                matrix,
                                jitter,
                                &depth,
                                size,
                                &indices
                            ));
                            assert!(motion.tagged().is_none());
                            assert!(motion.view().is_none());
                        }
                    }
                }
            }
        }
    }
    assert!(pollster::block_on(scope.pop()).is_none());
}
