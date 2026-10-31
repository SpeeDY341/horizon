//! Resolution scale, FXAA and TAA (Horizon settings): the world renders into its
//! own target at the scaled size, then one full-screen pass copies or
//! anti-aliases it onto the screen, under the hand and HUD. With none on,
//! the world renders straight to the screen.
//!
//! TAA jitters the world's projection by a sub-pixel offset each frame and blends
//! the frame into a history reprojected through the depth buffer: its motion
//! vectors are the camera's, so what moves on its own leans on the neighbourhood
//! clamp alone.

use glam::{DMat4, DVec3, Mat4, Vec2};

/// The TAA history format: 8 bits would band as frames accumulate.
const HISTORY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct WorldTarget {
    size: [u32; 2],
    color: wgpu::TextureView,
    /// Kept so Frame Generation can tag the image, not only the view.
    depth_texture: wgpu::Texture,
    depth: wgpu::TextureView,
    group: wgpu::BindGroup,
    taa: Option<History>,
}

/// TAA's two history textures at the output size (the world's, times the upscale), written
/// in turn: `resolve[i]` writes `history[i]` from the world and `history[1 - i]`; `show[i]`
/// copies `history[i]` to the screen.
struct History {
    size: [u32; 2],
    history: [wgpu::TextureView; 2],
    resolve: [wgpu::BindGroup; 2],
    show: [wgpu::BindGroup; 2],
}

/// `world_post.wgsl`'s `Taa`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TaaUniform {
    /// This frame's unjittered clip space to the previous frame's.
    reproject: [[f32; 4]; 4],
    /// xy: this frame's jitter in clip space; z: 1 when the history is unusable.
    jitter: [f32; 4],
}

/// TAA's GPU resources, absent when the device cannot compile the shader
/// (the OpenGL backend's naga GLSL emitter does not support `textureLoad`
/// from depth textures).
struct TaaResources {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
}

pub struct Post {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    copy: wgpu::RenderPipeline,
    fxaa: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    /// World resolution relative to the window (0.5..=2.0).
    pub scale: f32,
    pub antialias: bool,
    pub taa: bool,
    /// Output pixels per rendered pixel while TAA is on (1: native): the world renders
    /// this much smaller and TAA rebuilds it at the resolution scale's size.
    pub upscale: f32,
    internal_size: Option<[u32; 2]>,
    /// TAA runs this frame (`jitter`): not on the title screen or under a shader pack.
    taa_on: bool,
    /// Frame Generation without TAA still needs the previous camera. The picture
    /// itself stays unjittered.
    pub track_camera: bool,
    target: Option<WorldTarget>,
    taa_resources: Option<TaaResources>,
    /// Frames jittered so far (the Halton index), and which history the next resolve writes.
    taa_frame: u32,
    /// The last TAA frame's unjittered `view_proj` and camera, `None` once a frame ran without it.
    taa_previous: Option<(DMat4, DVec3)>,
    taa_this_frame: TaaUniform,
}

impl Post {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Horizon post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../world_post.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Horizon post bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Horizon post"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = |entry: &str, layout: &wgpu::PipelineLayout, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Horizon post"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_post"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // The OpenGL backend's GLSL translator does not support textureLoad
        // from depth textures: on devices running through OpenGL (such as older
        // Intel GPUs on Linux), TAA cannot compile its pipeline.
        let taa_resources = {
            let taa_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Horizon TAA bindings"),
                entries: &[
                    texture(0, float),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    texture(2, float),
                    texture(3, wgpu::TextureSampleType::Depth),
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
            let taa_pipeline_layout =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("Horizon TAA"),
                    bind_group_layouts: &[Some(&taa_layout)],
                    ..Default::default()
                });
            let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
            let taa_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Horizon TAA"),
                layout: Some(&taa_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_post"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_taa"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HISTORY_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            let internal = pollster::block_on(internal.pop());
            match internal.or(pollster::block_on(validation.pop())) {
                None => {
                    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Horizon TAA"),
                        size: std::mem::size_of::<TaaUniform>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    Some(TaaResources {
                        layout: taa_layout,
                        pipeline: taa_pipeline,
                        uniform,
                    })
                }
                Some(e) => {
                    log::warn!("Horizon renderer: temporal anti-aliasing unavailable ({e})");
                    None
                }
            }
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Horizon post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            copy: pipeline("fs_copy", &pipeline_layout, format),
            fxaa: pipeline("fs_fxaa", &pipeline_layout, format),
            layout,
            sampler,
            format,
            scale: 1.0,
            antialias: false,
            taa: false,
            upscale: 1.0,
            internal_size: None,
            taa_on: false,
            track_camera: false,
            target: None,
            taa_resources,
            taa_frame: 0,
            taa_previous: None,
            taa_this_frame: bytemuck::Zeroable::zeroed(),
        }
    }

    /// Whether TAA is supported on this GPU/backend.
    pub fn supports_taa(&self) -> bool {
        self.taa_resources.is_some()
    }

    /// The world's rendered size and the size it is shown at (TAA's history).
    fn sizes(&self, window: [u32; 2]) -> ([u32; 2], [u32; 2]) {
        let output = window.map(|v| ((v as f32 * self.scale).round() as u32).clamp(1, 8192));
        let upscale = if self.taa_on { self.upscale } else { 1.0 };
        (
            self.internal_size
                .filter(|_| self.taa_on)
                .unwrap_or_else(|| output.map(|v| ((v as f32 / upscale).round() as u32).max(1))),
            output,
        )
    }

    pub fn set_internal_size(&mut self, size: Option<[u32; 2]>) {
        if self.internal_size != size {
            self.taa_previous = None;
            self.internal_size = size;
        }
    }
    pub fn invalidate_history(&mut self) {
        self.taa_this_frame.jitter[2] = 1.0;
    }
    pub fn reprojection(&self) -> Mat4 {
        Mat4::from_cols_array_2d(&self.taa_this_frame.reproject)
    }

    /// TAA's sub-pixel jitter this frame, as a clip-space matrix to multiply into the
    /// world's projection (identity while `on` is false); `view_proj` is the unjittered
    /// projection, relative to `camera`. Call it before `prepare`, once per frame.
    pub fn jitter(&mut self, window: [u32; 2], on: bool, view_proj: Mat4, camera: DVec3) -> Mat4 {
        self.taa_on = on && self.taa && self.taa_resources.is_some();
        let track = self.track_camera && !self.taa_on;
        if !self.taa_on && !track {
            self.taa_previous = None;
            return Mat4::IDENTITY;
        }
        if track {
            let current = view_proj.as_dmat4();
            let (reproject, reset) = match self.taa_previous {
                Some((previous, previous_camera)) => (
                    reprojection(previous, previous_camera, current, camera),
                    0.0,
                ),
                None => (DMat4::IDENTITY, 1.0),
            };
            self.taa_previous = Some((current, camera));
            self.taa_this_frame = TaaUniform {
                reproject: reproject.as_mat4().to_cols_array_2d(),
                jitter: [0.0, 0.0, reset, 0.0],
            };
            return Mat4::IDENTITY;
        }
        self.taa_frame = self.taa_frame.wrapping_add(1);
        let (size, _) = self.sizes(window);
        // Halton (2, 3) over 8 frames per output pixel (FSR's count), in rendered
        // pixels around the centre.
        let ratio = self.internal_size.map_or(self.upscale, |_| {
            self.sizes(window).1[0] as f32 / size[0] as f32
        });
        let phases = (8.0 * ratio * ratio).ceil() as u32;
        let i = self.taa_frame % phases + 1;
        let offset = Vec2::new(halton(i, 2), halton(i, 3)) - 0.5;
        let jitter = offset * 2.0 / Vec2::new(size[0] as f32, size[1] as f32);
        let current = view_proj.as_dmat4();
        let (reproject, reset) = match self.taa_previous {
            Some((previous, previous_camera)) => (
                reprojection(previous, previous_camera, current, camera),
                0.0,
            ),
            None => (DMat4::IDENTITY, 1.0),
        };
        self.taa_previous = Some((current, camera));
        self.taa_this_frame = TaaUniform {
            reproject: reproject.as_mat4().to_cols_array_2d(),
            jitter: [jitter.x, jitter.y, reset, 0.0],
        };
        Mat4::from_translation(jitter.extend(0.0))
    }

    /// The world's colour and depth targets this frame, or `None` to draw
    /// it straight to the screen.
    /// `always`: a target even at scale 1 without anti-aliasing (a shader pack reads it).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        window: [u32; 2],
        always: bool,
    ) -> Option<(wgpu::TextureView, wgpu::TextureView)> {
        if self.scale == 1.0 && !self.antialias && !self.taa_on && !always {
            self.target = None;
            return None;
        }
        let (size, output) = self.sizes(window);
        let history_size = self.taa_on.then_some(output);
        if self
            .target
            .as_ref()
            .is_none_or(|t| t.size != size || t.taa.as_ref().map(|h| h.size) != history_size)
        {
            let make_tex = |label, size: [u32; 2], format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
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
            let sampled =
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            let color = make_tex("Scaled world", size, self.format, sampled)
                .create_view(&Default::default());
            // TEXTURE_BINDING: occlusion culling and TAA read it. COPY_SRC: Frame
            // Generation copies it before the hand clears the screen depth.
            let depth_texture = make_tex(
                "Scaled world depth",
                size,
                wgpu::TextureFormat::Depth32Float,
                sampled | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = depth_texture.create_view(&Default::default());
            let group = self.bind(device, &color);
            let taa = history_size.and_then(|output| {
                let res = self.taa_resources.as_ref()?;
                let history = [0, 1].map(|_| {
                    make_tex("TAA history", output, HISTORY_FORMAT, sampled)
                        .create_view(&Default::default())
                });
                let resolve = [0, 1].map(|i| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Horizon TAA"),
                        layout: &res.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&color),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&history[1 - i]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(&depth),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: res.uniform.as_entire_binding(),
                            },
                        ],
                    })
                });
                let show = [0, 1].map(|i| self.bind(device, &history[i]));
                Some(History {
                    size: output,
                    history,
                    resolve,
                    show,
                })
            });
            // New textures: no history yet.
            self.taa_this_frame.jitter[2] = 1.0;
            self.target = Some(WorldTarget {
                size,
                color,
                depth_texture,
                depth,
                group,
                taa,
            });
        }
        self.target
            .as_ref()
            .map(|t| (t.color.clone(), t.depth.clone()))
    }

    /// Exactly the jitter used for this frame's world, including target rebuild resets.
    pub fn temporal_jitter(&self) -> Option<[f32; 4]> {
        (self.taa_on || self.track_camera).then_some(self.taa_this_frame.jitter)
    }

    /// The size of the world's own target, when it has one.
    pub fn size(&self) -> Option<[u32; 2]> {
        self.target.as_ref().map(|t| t.size)
    }

    /// Depth image and view for a Frame Generation tag. The view is the one the world wrote.
    pub fn depth_inputs(&self) -> Option<(wgpu::Texture, wgpu::TextureView)> {
        self.target
            .as_ref()
            .map(|t| (t.depth_texture.clone(), t.depth.clone()))
    }

    /// Bindings that copy (or anti-alias) `view` onto the screen in `finish_with`.
    pub fn bind(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Horizon post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Draw the scaled world onto `screen`, through TAA when this frame's `jitter` had it on.
    pub fn finish(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        screen: &wgpu::TextureView,
    ) {
        let Some(target) = &self.target else { return };
        let Some(taa) = target.taa.as_ref().filter(|_| self.taa_previous.is_some()) else {
            self.finish_with(encoder, screen, &target.group);
            return;
        };
        let Some(res) = &self.taa_resources else {
            self.finish_with(encoder, screen, &target.group);
            return;
        };
        let i = (self.taa_frame % 2) as usize;
        queue.write_buffer(&res.uniform, 0, bytemuck::bytes_of(&self.taa_this_frame));
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &taa.history[i],
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Horizon TAA"),
            color_attachments: &attachments,
            ..Default::default()
        });
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &taa.resolve[i], &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        self.finish_with(encoder, screen, &taa.show[i]);
    }

    /// Draw what `group` binds (`bind`) onto `screen`.
    pub fn finish_with(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        screen: &wgpu::TextureView,
        group: &wgpu::BindGroup,
    ) {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: screen,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Horizon post"),
            color_attachments: &attachments,
            ..Default::default()
        });
        pass.set_pipeline(if self.antialias {
            &self.fxaa
        } else {
            &self.copy
        });
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// This frame's clip space to the previous frame's, for camera-relative projections:
/// a point `camera - previous_camera` further from the previous camera than from this one.
fn reprojection(previous: DMat4, previous_camera: DVec3, current: DMat4, camera: DVec3) -> DMat4 {
    previous * DMat4::from_translation(camera - previous_camera) * current.inverse()
}

/// The radical inverse of `i` in `base`: a low-discrepancy sequence in 0..1.
fn halton(mut i: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0, 0.0);
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reprojection_finds_where_a_block_was_last_frame() {
        let projection = DMat4::perspective_rh(1.2, 16.0 / 9.0, 1024.0, 0.05);
        let previous = projection * DMat4::from_rotation_y(0.3);
        let current = projection * DMat4::from_rotation_y(0.35);
        let (previous_camera, camera) = (DVec3::new(10.0, 64.0, 5.0), DVec3::new(10.5, 64.2, 4.0));
        let block = DVec3::new(14.0, 66.0, -8.0);
        let clip = |m: DMat4, eye: DVec3| m * (block - eye).extend(1.0);
        let now = clip(current, camera);
        let then = reprojection(previous, previous_camera, current, camera) * (now / now.w);
        let expected = clip(previous, previous_camera);
        assert!((then.truncate() / then.w - expected.truncate() / expected.w).length() < 1e-9);
    }

    #[test]
    fn jitter_stays_inside_the_pixel() {
        for i in 1..=8 {
            let (x, y) = (halton(i, 2), halton(i, 3));
            assert!((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y));
        }
        assert_eq!(
            [halton(1, 2), halton(2, 2), halton(3, 2)],
            [0.5, 0.25, 0.75]
        );
    }

    #[test]
    fn frame_generation_tracks_the_camera_without_jittering_the_picture() {
        let Some((device, _)) = test_device() else {
            return;
        };
        let mut post = Post::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        post.track_camera = true;
        let view = Mat4::from_translation(glam::Vec3::new(1.0, 0.0, 0.0));
        let jittered = post.jitter([100, 100], false, view, DVec3::ZERO);
        assert_eq!(jittered, Mat4::IDENTITY);
        let first = post.temporal_jitter().unwrap();
        assert_eq!(first[0], 0.0);
        assert_eq!(first[2], 1.0);
        let jittered = post.jitter([100, 100], false, view, DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(jittered, Mat4::IDENTITY);
        assert_eq!(post.temporal_jitter().unwrap()[2], 0.0);
        assert_ne!(post.reprojection(), Mat4::IDENTITY);
    }

    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    #[test]
    fn world_post_pipelines_compile_or_fall_back() {
        let Some((device, _)) = test_device() else {
            return;
        };
        let post = Post::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        // Copy and FXAA pipelines always compile; TAA compiles if supported by backend.
        let _ = post.supports_taa();
    }
}
