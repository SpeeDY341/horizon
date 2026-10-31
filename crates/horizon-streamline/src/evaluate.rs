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
        if unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.is_some() {
            self.record_directx12(&mut encoder, &textures, evaluate)?;
        } else {
            self.record_vulkan(&mut encoder, &textures, evaluate)?;
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

    fn record_vulkan(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        textures: &[&wgpu::Texture; 4],
        evaluate: impl FnOnce(*mut c_void, &mut [sl::Resource; 4]) -> Result<(), String>,
    ) -> Result<(), String> {
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
            // Only the output can be shared with the caller. Retain its guard
            // during DLSS so it cannot be snatched out from under the call.
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
            let mut resources = std::array::from_fn(|i| {
                tagged_resource(
                    images[i].as_raw() as *mut c_void,
                    raw_views[i].as_raw() as *mut c_void,
                    layouts[i].as_raw() as u32,
                    textures[i].width(),
                    textures[i].height(),
                    formats[i].as_raw() as u32,
                    usages[i].as_raw(),
                )
            });
            encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(|hal| {
                let hal = hal.ok_or("Vulkan command encoder missing")?;
                evaluate(hal.raw_handle().as_raw() as *mut c_void, &mut resources)
            })?
        }
        Ok(())
    }

    fn record_directx12(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        textures: &[&wgpu::Texture; 4],
        evaluate: impl FnOnce(*mut c_void, &mut [sl::Resource; 4]) -> Result<(), String>,
    ) -> Result<(), String> {
        // These match wgpu-hal's barriers for TextureUses::RESOURCE and
        // STORAGE_READ_WRITE. The depth copy is sampled, so it is a shader
        // resource (0xC0), not a depth-read target. Views are not required.
        const SHADER_RESOURCE: u32 = 0x40 | 0x80;
        const UNORDERED_ACCESS: u32 = 0x8;
        const R16G16B16A16_FLOAT: u32 = 10;
        const R16G16_FLOAT: u32 = 34;
        const D32_FLOAT: u32 = 40;
        let states = [
            SHADER_RESOURCE,
            SHADER_RESOURCE,
            SHADER_RESOURCE,
            UNORDERED_ACCESS,
        ];
        let formats = [
            R16G16B16A16_FLOAT,
            D32_FLOAT,
            R16G16_FLOAT,
            R16G16B16A16_FLOAT,
        ];
        // Safety: textures belong to this device. Pointers stay valid while
        // the textures do. The command list stays open for wgpu to close.
        unsafe {
            let mut images = [std::ptr::null_mut(); 4];
            for (i, texture) in textures.iter().enumerate() {
                let hal = texture
                    .as_hal::<wgpu::hal::api::Dx12>()
                    .ok_or("DirectX 12 texture missing")?;
                images[i] = hal.raw_resource_ptr();
                if images[i].is_null() {
                    return Err("DirectX 12 texture missing".into());
                }
            }
            let _output_guard = self
                .output
                .as_hal::<wgpu::hal::api::Dx12>()
                .ok_or("DirectX 12 output missing")?;
            let mut resources = std::array::from_fn(|i| {
                tagged_resource(
                    images[i],
                    std::ptr::null_mut(),
                    states[i],
                    textures[i].width(),
                    textures[i].height(),
                    formats[i],
                    0,
                )
            });
            encoder.as_hal_mut::<wgpu::hal::api::Dx12, _, _>(|hal| {
                let hal = hal.ok_or("DirectX 12 command encoder missing")?;
                let list = hal
                    .raw_open_command_list_ptr()
                    .ok_or("DirectX 12 command list is not open")?;
                evaluate(list, &mut resources)
            })?
        }
        Ok(())
    }
}

fn tagged_resource(
    native: *mut c_void,
    view: *mut c_void,
    state: u32,
    width: u32,
    height: u32,
    format: u32,
    usage: u32,
) -> sl::Resource {
    sl::Resource {
        base: Base::new(
            0x3a9d70cf,
            0x2418,
            0x4b72,
            [0x83, 0x91, 0x13, 0xf8, 0x72, 0x1c, 0x72, 0x61],
            1,
        ),
        kind: 0,
        native,
        memory: std::ptr::null_mut(),
        view,
        state,
        width,
        height,
        format,
        mips: 1,
        layers: 1,
        gpu_address: 0,
        flags: 0,
        usage,
        internal_flags: 0,
        reserved: 0,
    }
}
