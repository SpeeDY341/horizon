//! Optional DLSS inputs. Never changes the existing TAA resolve.
use super::{
    Vertex,
    motion_history::{History, Span},
};
use glam::{DMat4, DVec3, Mat4};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    current: [[f32; 4]; 4],
    previous: [[f32; 4]; 4],
    reproject: [[f32; 4]; 4],
    jitter: [f32; 4],
}

pub(super) struct Motion {
    camera: wgpu::RenderPipeline,
    entity: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    atlas: wgpu::TextureView,
    sampler: wgpu::Sampler,
    target: Option<wgpu::Texture>,
    history: History,
    previous: Option<(Mat4, DVec3)>,
    frame_reset: bool,
}

impl Motion {
    pub fn new(device: &wgpu::Device, atlas: &wgpu::TextureView) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DLSS motion"),
            source: wgpu::ShaderSource::Wgsl(include_str!("motion.wgsl").into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let texture = |sample_type| wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("DLSS motion"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, texture(wgpu::TextureSampleType::Depth)),
                entry(
                    2,
                    texture(wgpu::TextureSampleType::Float { filterable: true }),
                ),
                entry(
                    3,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("DLSS motion"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let attributes =
            wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32,4=>Float32];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        })];
        let pipeline = |entity: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("DLSS motion"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(if entity { "vs_entity" } else { "vs_camera" }),
                    compilation_options: Default::default(),
                    buffers: if entity { &buffers } else { &[] },
                },
                primitive: Default::default(),
                multisample: Default::default(),
                depth_stencil: entity.then_some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if entity { "fs_entity" } else { "fs_camera" }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rg16Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            camera: pipeline(false),
            entity: pipeline(true),
            layout,
            uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("DLSS motion"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            atlas: atlas.clone(),
            sampler: device.create_sampler(&Default::default()),
            target: None,
            history: History::default(),
            previous: None,
            frame_reset: true,
        }
    }

    pub fn view(&self) -> Option<wgpu::TextureView> {
        self.target
            .as_ref()
            .map(|t| t.create_view(&Default::default()))
    }
    pub fn frame_reset(&self) -> bool {
        self.frame_reset
    }

    pub fn reset(&mut self) {
        self.frame_reset = true;
        self.history.clear();
        self.previous = None;
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera_position: DVec3,
        spans: &[Span],
        entities: &[Vertex],
        current: Mat4,
        jitter: [f32; 4],
        depth: &wgpu::TextureView,
        size: [u32; 2],
        indices: &wgpu::Buffer,
    ) {
        if self
            .target
            .as_ref()
            .is_none_or(|t| [t.width(), t.height()] != size)
        {
            self.reset();
            self.target = Some(device.create_texture(&wgpu::TextureDescriptor {
                label: Some("DLSS motion"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));
        }
        let reset = jitter[2] != 0.0
            || self
                .previous
                .is_none_or(|(_, c)| c.distance_squared(camera_position) > 1024.0);
        if reset {
            self.reset();
        }
        self.frame_reset = reset;
        let (previous, camera) = self.previous.unwrap_or((current, camera_position));
        let reproject = previous.as_dmat4()
            * DMat4::from_translation(camera_position - camera)
            * current.as_dmat4().inverse();
        let u = Uniform {
            current: current.to_cols_array_2d(),
            previous: previous.to_cols_array_2d(),
            reproject: reproject.as_mat4().to_cols_array_2d(),
            jitter: [jitter[0], jitter[1], if reset { 1.0 } else { 0.0 }, 0.0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
        self.history.update(spans, entities, camera_position, reset);
        self.previous = Some((current, camera_position));
        let target = self
            .target
            .as_ref()
            .unwrap()
            .create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DLSS motion"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.atlas),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("DLSS camera motion"),
                color_attachments: &attachments,
                ..Default::default()
            });
            pass.set_pipeline(&self.camera);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        if self.history.vertices.is_empty() {
            return;
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("DLSS entity motion"),
            contents: bytemuck::cast_slice(&self.history.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("DLSS object motion"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: None,
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&self.entity);
        pass.set_bind_group(0, &group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..(self.history.vertices.len() / 4 * 6) as u32, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn motion_pipelines_compile_on_vulkan() {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(desc);
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
            return;
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut motion = super::Motion::new(&device, &atlas.create_view(&Default::default()));
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth = depth.create_view(&Default::default());
        let vertices = [super::Vertex {
            pos: [0.0, 0.0, 0.5],
            color: u32::MAX,
            ..bytemuck::Zeroable::zeroed()
        }; 4];
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 24,
            usage: wgpu::BufferUsages::INDEX,
            mapped_at_creation: false,
        });
        for i in 0..2 {
            let mut encoder = device.create_command_encoder(&Default::default());
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[],
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
            motion.render(
                &device,
                &queue,
                &mut encoder,
                glam::DVec3::ZERO,
                &[super::Span {
                    id: 1,
                    item: false,
                    vertices: 0..4,
                }],
                &vertices,
                glam::Mat4::IDENTITY,
                [0.0, 0.0, if i == 0 { 1.0 } else { 0.0 }, 0.0],
                &depth,
                [8, 8],
                &indices,
            );
            queue.submit([encoder.finish()]);
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert!(pollster::block_on(scope.pop()).is_none());
    }

    #[test]
    fn motion_shader_validates() {
        let module = naga::front::wgsl::parse_str(include_str!("motion.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
