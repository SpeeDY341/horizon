use crate::{abi::Base, dlss_abi as sl};
use ash::vk::{self, Handle};
use std::ffi::c_void;

pub(crate) struct Targets {
    pub size: [u32; 2],
    color: wgpu::Texture,
    depth: wgpu::Texture,
    motion: wgpu::Texture,
    pub output: wgpu::Texture,
    copy: wgpu::RenderPipeline,
    views: Option<[wgpu::TextureView; 4]>,
}
impl Targets {
    pub fn new(device: &wgpu::Device, size: [u32; 2], output: [u32; 2]) -> Self {
        let make = |label, size: [u32; 2], format, usage| {
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
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("DLSS input copy"),
            source: wgpu::ShaderSource::Wgsl(include_str!("copy.wgsl").into()),
        });
        let copy = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("DLSS input copy"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mut targets = Self {
            size,
            copy,
            views: None,
            color: make(
                "DLSS color",
                size,
                wgpu::TextureFormat::Rgba16Float,
                sampled | wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
            depth: make(
                "DLSS depth",
                size,
                wgpu::TextureFormat::Depth32Float,
                sampled | wgpu::TextureUsages::COPY_DST,
            ),
            motion: make(
                "DLSS motion",
                size,
                wgpu::TextureFormat::Rg16Float,
                sampled | wgpu::TextureUsages::COPY_DST,
            ),
            output: make(
                "DLSS output",
                output,
                wgpu::TextureFormat::Rgba16Float,
                sampled
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
        };
        targets.views = Some(
            [
                &targets.color,
                &targets.depth,
                &targets.motion,
                &targets.output,
            ]
            .map(|t| t.create_view(&Default::default())),
        );
        targets
    }

    pub fn copy_inputs(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        motion: &wgpu::TextureView,
        timing: Option<(&crate::monitor::Monitor, usize)>,
    ) -> Result<(), String> {
        for source in [color, depth, motion] {
            if [source.texture().width(), source.texture().height()] != self.size {
                return Err("DLSS input sizes differ".into());
            }
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = device.create_command_encoder(&Default::default());
        if let Some((m, slot)) = timing {
            m.stamp(&mut encoder, slot, 0);
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DLSS input copy"),
            layout: &self.copy.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(color),
            }],
        });
        let target = self.color.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("DLSS color conversion"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.copy);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        for (source, target) in [(depth, &self.depth), (motion, &self.motion)] {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: source.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: target,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                target.size(),
            );
        }
        // Mark output initialized in wgpu before an external writer touches it.
        // Otherwise wgpu's first subsequent read clears DLSS's first result.
        let output = self.output.create_view(&Default::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("DLSS output initialization"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        }));
        if let Some((m, slot)) = timing {
            m.stamp(&mut encoder, slot, 1);
        }
        let commands = encoder.finish();
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(e.to_string());
        }
        queue.submit([commands]);
        Ok(())
    }

    pub fn evaluate_native(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        timing: Option<(&crate::monitor::Monitor, usize)>,
        evaluate: impl FnOnce(*mut c_void, &mut [sl::Resource; 4]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut encoder = device.create_command_encoder(&Default::default());
        let textures = [&self.color, &self.depth, &self.motion, &self.output];
        encoder.transition_resources(
            std::iter::empty(),
            textures
                .iter()
                .enumerate()
                .map(|(i, t)| wgpu::TextureTransition {
                    texture: *t,
                    selector: None,
                    state: if i == 3 {
                        wgpu::TextureUses::STORAGE_READ_WRITE
                    } else {
                        wgpu::TextureUses::RESOURCE
                    },
                }),
        );
        if let Some((m, slot)) = timing {
            m.stamp(&mut encoder, slot, 2);
        }
        let before = encoder.finish();
        let mut encoder = device.create_command_encoder(&Default::default());
        let views = self.views.as_ref().unwrap();
        // Safety: these four textures were allocated on this device by this
        // package. Guards retain them throughout the call, and no render pass
        // is open. DLSS receives their exact current Vulkan states.
        unsafe {
            let images =
                textures.map(|t| t.as_hal::<wgpu::hal::api::Vulkan>().unwrap().raw_handle());
            let raw_views = views
                .each_ref()
                .map(|v| v.as_hal::<wgpu::hal::api::Vulkan>().unwrap().raw_handle());
            // wgpu's snatch lock is device-wide and non-reentrant. The three
            // inputs and all views are private to Targets; only the output can
            // be shared with the caller. Retain its single guard during DLSS.
            let _output_guard = self.output.as_hal::<wgpu::hal::api::Vulkan>().unwrap();
            let formats = [
                vk::Format::R16G16B16A16_SFLOAT,
                vk::Format::D32_SFLOAT,
                vk::Format::R16G16_SFLOAT,
                vk::Format::R16G16B16A16_SFLOAT,
            ];
            let layouts = [
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::GENERAL,
            ];
            let usages = [
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::COLOR_ATTACHMENT,
            ];
            let mut resources = std::array::from_fn(|i| sl::Resource {
                base: Base::new(
                    0x3a9d70cf,
                    0x2418,
                    0x4b72,
                    [0x83, 0x91, 0x13, 0xf8, 0x72, 0x1c, 0x72, 0x61],
                    1,
                ),
                kind: 0,
                native: images[i].as_raw() as *mut c_void,
                memory: std::ptr::null_mut(),
                view: raw_views[i].as_raw() as *mut c_void,
                state: layouts[i].as_raw() as u32,
                width: textures[i].width(),
                height: textures[i].height(),
                format: formats[i].as_raw() as u32,
                mips: 1,
                layers: 1,
                gpu_address: 0,
                flags: 0,
                usage: usages[i].as_raw(),
                internal_flags: 0,
                reserved: 0,
            });
            encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(|hal| {
                let hal = hal.ok_or("Vulkan command encoder missing")?;
                evaluate(hal.raw_handle().as_raw() as *mut c_void, &mut resources)
            })?;
        }
        let native_commands = encoder.finish();
        let mut encoder = device.create_command_encoder(&Default::default());
        // DLSS restores the tagged input states. wgpu resumes ownership here.
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &self.output,
                selector: None,
                state: wgpu::TextureUses::RESOURCE,
            }),
        );
        if let Some((m, slot)) = timing {
            m.end(&mut encoder, slot);
        }
        queue.submit([before, native_commands, encoder.finish()]);
        if let Some((m, slot)) = timing {
            m.submitted(slot);
        }
        Ok(())
    }
}
