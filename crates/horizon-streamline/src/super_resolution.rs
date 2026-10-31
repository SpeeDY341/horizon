//! DLSS owns its inputs. Native code never receives arbitrary engine handles.
use crate::{Runtime, abi::Base, dlss_abi as sl};
use std::{
    ffi::{CStr, c_void},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameConstants {
    /// Row-vector, row-major matrices, without jitter.
    pub projection: [[f32; 4]; 4],
    pub inverse_projection: [[f32; 4]; 4],
    pub reprojection: [[f32; 4]; 4],
    pub inverse_reprojection: [[f32; 4]; 4],
    pub jitter_pixels: [f32; 2],
    pub camera: [[f32; 3]; 4],
    pub near: f32,
    pub far: f32,
    pub fov: f32,
    pub aspect: f32,
    pub reset: bool,
}
impl FrameConstants {
    pub(crate) fn finite(&self) -> bool {
        self.projection
            .iter()
            .flatten()
            .chain(self.inverse_projection.iter().flatten())
            .chain(self.reprojection.iter().flatten())
            .chain(self.inverse_reprojection.iter().flatten())
            .chain(self.jitter_pixels.iter())
            .chain(self.camera.iter().flatten())
            .chain([self.near, self.far, self.fov, self.aspect].iter())
            .all(|v| v.is_finite())
    }
}

struct Api {
    optimal: sl::GetOptimal,
    options: sl::SetOptions,
    evaluate: sl::Evaluate,
}
impl Api {
    fn load(runtime: &Runtime) -> Result<Self, String> {
        // Safety: exact pinned SDK ABI; runtime outlives these function pointers.
        unsafe {
            let get = *runtime
                ._library
                .get::<sl::GetFunction>(b"slGetFeatureFunction\0")
                .map_err(|e| e.to_string())?;
            let function = |name: &CStr| -> Result<*mut c_void, String> {
                let mut p = std::ptr::null_mut();
                check(get(0, name.as_ptr(), &mut p))?;
                if p.is_null() {
                    return Err("Null DLSS function".into());
                }
                Ok(p)
            };
            Ok(Self {
                optimal: std::mem::transmute::<*mut c_void, sl::GetOptimal>(function(
                    c"slDLSSGetOptimalSettings",
                )?),
                options: std::mem::transmute::<*mut c_void, sl::SetOptions>(function(
                    c"slDLSSSetOptions",
                )?),
                evaluate: *runtime
                    ._library
                    .get::<sl::Evaluate>(b"slEvaluateFeature\0")
                    .map_err(|e| e.to_string())?,
            })
        }
    }
}
/// Safety: callers hold the guards only to compare identities. The handles
/// remain valid because `device`, `queue` and the registered device are alive.
unsafe fn same_device(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    owner: &Runtime,
) -> Result<(), String> {
    let registered = owner
        .registered_device()
        .ok_or("DLSS device is not registered")?;
    unsafe {
        if let Some(d) = device.as_hal::<wgpu::hal::api::Vulkan>() {
            let q = queue
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("Vulkan queue missing")?;
            let registered = registered
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("Registered Vulkan device missing")?;
            if d.raw_device().handle() != registered.raw_device().handle() {
                return Err("DLSS runtime/device mismatch".into());
            }
            if d.raw_device().handle() != q.raw_device().handle() {
                return Err("DLSS queue/device mismatch".into());
            }
            return Ok(());
        }
        if let Some(d) = device.as_hal::<wgpu::hal::api::Dx12>() {
            let q = queue
                .as_hal::<wgpu::hal::api::Dx12>()
                .ok_or("DirectX 12 queue missing")?;
            let registered = registered
                .as_hal::<wgpu::hal::api::Dx12>()
                .ok_or("Registered DirectX 12 device missing")?;
            let ptr = d.raw_device_ptr();
            if ptr.is_null() || ptr != registered.raw_device_ptr() {
                return Err("DLSS runtime/device mismatch".into());
            }
            if ptr != q.raw_device_ptr() {
                return Err("DLSS queue/device mismatch".into());
            }
            return Ok(());
        }
    }
    Err("DLSS device is neither Vulkan nor DirectX 12".into())
}
/// Common constants for one presented frame. Matrices stay free of jitter.
/// Depth is reversed and motion already includes the camera, matching DLSS.
pub(crate) fn native_constants(frame: &FrameConstants) -> sl::Constants {
    let identity = [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ];
    sl::Constants {
        base: Base::new(
            0xdcd35ad7,
            0x4e4a,
            0x4bad,
            [0xa9, 0x0c, 0xe0, 0xc4, 0x9e, 0xb2, 0x3a, 0xfe],
            2,
        ),
        matrices: [
            frame.projection,
            frame.inverse_projection,
            identity,
            frame.reprojection,
            frame.inverse_reprojection,
        ],
        jitter: frame.jitter_pixels,
        motion_scale: [1., 1.],
        pinhole: [0.; 2],
        camera: frame.camera,
        near: frame.near,
        far: frame.far,
        fov: frame.fov,
        aspect: frame.aspect,
        invalid_motion: f32::MIN,
        flags: [1, 1, 0, u8::from(frame.reset), 0, 0, 0],
        separation: 40.,
    }
}

fn check(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("NVIDIA DLSS error {code}"))
    }
}

pub struct SuperResolution {
    api: Api,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub(crate) targets: Option<super::evaluate::Targets>,
    configuration: Option<(u32, [u32; 2], u32)>,
    monitor: Option<crate::monitor::Monitor>,
    performance: crate::Performance,
    // Last: shutdown must run with the device and instance still retained by Runtime.
    _runtime: Arc<Mutex<Runtime>>,
}

impl SuperResolution {
    pub fn new(
        runtime: Arc<Mutex<Runtime>>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Self, String> {
        let owner = runtime.lock().map_err(|e| e.to_string())?;
        // Safety: read-only guards verify the queue and device match before copying.
        unsafe { same_device(device, queue, &owner)? };
        let api = Api::load(&owner)?;
        drop(owner);
        Ok(Self {
            api,
            device: device.clone(),
            queue: queue.clone(),
            targets: None,
            configuration: None,
            monitor: None,
            performance: crate::Performance::default(),
            _runtime: runtime,
        })
    }

    pub fn set_monitoring(&mut self, enabled: bool) {
        if enabled && self.monitor.is_none() {
            self.monitor = crate::monitor::Monitor::new(&self.device, &self.queue);
        } else if !enabled {
            self.monitor = None;
        }
    }
    pub fn performance(&self) -> crate::Performance {
        self.performance
    }

    /// Quality, Balanced, Performance, Ultra Performance, as the SDK reports them.
    /// Each entry is optimal, minimum, maximum render size.
    pub fn recommended_sizes(
        &self,
        output: [u32; 2],
        preset: u32,
    ) -> Result<[[[u32; 2]; 3]; 4], String> {
        let _owner = self._runtime.lock().map_err(|e| e.to_string())?;
        let mut sizes = [[[0; 2]; 3]; 4];
        for (at, mode) in [3, 2, 1, 4].into_iter().enumerate() {
            let mut options = sl::Options::new(mode, output);
            options.presets = [preset; 6];
            let mut size = sl::Optimal::new();
            // Safety: live plugin, initialized ABI structures; session lock held.
            unsafe {
                check((self.api.optimal)(&options, &mut size))?;
            }
            sizes[at] = [
                [size.width, size.height],
                [size.min_width, size.min_height],
                [size.max_width, size.max_height],
            ];
        }
        Ok(sizes)
    }

    /// Mode values are the SDK's Quality=3, Balanced=2, Performance=1,
    /// Ultra Performance=4, DLAA=6. Size is obtained from NVIDIA, never guessed.
    pub fn configure(&mut self, mode: u32, output: [u32; 2]) -> Result<[u32; 2], String> {
        self.configure_with_preset(mode, output, 0)
    }

    /// A requested SDK preset. The SDK/driver can override legacy presets.
    pub fn configure_with_preset(
        &mut self,
        mode: u32,
        output: [u32; 2],
        preset: u32,
    ) -> Result<[u32; 2], String> {
        let _session = self._runtime.lock().map_err(|e| e.to_string())?;
        _session.presentation_ready()?;
        if ![0, 5, 6, 10, 11, 12, 13].contains(&preset)
            || ![1, 2, 3, 4, 6].contains(&mode)
            || output.contains(&0)
            || output.iter().any(|n| *n > 8192)
        {
            return Err("Invalid DLSS mode or output size".into());
        }
        if self.configuration == Some((mode, output, preset)) {
            return Ok(self.targets.as_ref().unwrap().size);
        }
        let viewport = sl::Viewport::new();
        let mut options = sl::Options::new(mode, output);
        options.presets = [preset; 6];
        let mut optimal = sl::Optimal::new();
        // Safety: initialized DLSS on a registered Vulkan device and pinned ABI.
        unsafe {
            check((self.api.optimal)(&options, &mut optimal))?;
            check((self.api.options)(&viewport, &options))?;
        }
        let size = [optimal.width, optimal.height];
        if size.contains(&0) || size.iter().zip(output).any(|(a, b)| *a > b) {
            return Err("Invalid DLSS render size".into());
        }
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let targets = super::evaluate::Targets::new(&self.device, size, output);
        let memory = pollster::block_on(memory.pop());
        let internal = pollster::block_on(internal.pop());
        let validation = pollster::block_on(validation.pop());
        if let Some(e) = memory.or(internal).or(validation) {
            return Err(e.to_string());
        }
        self.targets = Some(targets);
        self.configuration = Some((mode, output, preset));
        Ok(size)
    }

    /// Caller has submitted world rendering on this device's queue. Input
    /// resources are copied and validated by wgpu before native DLSS runs.
    pub fn evaluate(
        &mut self,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        motion: &wgpu::TextureView,
        frame: FrameConstants,
    ) -> Result<wgpu::TextureView, String> {
        // Standalone probes have no renderer lifecycle. Each evaluate is one
        // complete frame there; the game opens a shared SR/FG frame explicitly.
        let standalone = {
            let mut runtime = self._runtime.lock().map_err(|e| e.to_string())?;
            let standalone = !runtime.render_frame_active();
            if standalone {
                runtime.begin_render_frame();
            }
            standalone
        };
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let started = std::time::Instant::now();
        let mut result = self.evaluate_frame(color, depth, motion, frame);
        let memory = pollster::block_on(memory.pop());
        let internal = pollster::block_on(internal.pop());
        let validation = pollster::block_on(validation.pop());
        if let Some(e) = memory.or(internal).or(validation) {
            result = Err(e.to_string());
        }
        {
            let mut runtime = self._runtime.lock().map_err(|e| e.to_string())?;
            if result.is_err() {
                runtime.fail_render_frame();
            }
            if standalone {
                runtime.end_render_frame(result.is_ok());
            }
        }
        self.performance.cpu_total_ms = started.elapsed().as_secs_f32() * 1000.0;
        result
    }

    fn evaluate_frame(
        &mut self,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        motion: &wgpu::TextureView,
        frame: FrameConstants,
    ) -> Result<wgpu::TextureView, String> {
        let mut _session = self._runtime.lock().map_err(|e| e.to_string())?;
        _session.presentation_ready()?;
        let viewport = sl::Viewport::new();
        let targets = self.targets.as_ref().ok_or("DLSS is not configured")?;
        let slot = self.monitor.as_mut().and_then(|m| m.begin(&self.device));
        let timing = self.monitor.as_ref().zip(slot);
        let started = std::time::Instant::now();
        if let Err(e) = targets.copy_inputs(&self.device, &self.queue, color, depth, motion, timing)
        {
            if let Some((m, slot)) = timing {
                m.cancel(slot);
            }
            return Err(e);
        }
        self.performance.cpu_copy_ms = started.elapsed().as_secs_f32() * 1000.0;
        let started = std::time::Instant::now();
        let token = _session.common_constants(frame)?;
        let result =
            targets.evaluate_native(&self.device, &self.queue, timing, |cmd, resources| {
                let mut tags = resources
                    .iter_mut()
                    .zip([3, 0, 1, 4])
                    .map(|(r, kind)| sl::Tag {
                        base: Base::new(
                            0x4c6a5aad,
                            0xb445,
                            0x496c,
                            [0x87, 0xff, 0x1a, 0xf3, 0x84, 0x5b, 0xe6, 0x53],
                            1,
                        ),
                        extent: [0, 0, r.width, r.height],
                        resource: r,
                        kind,
                        lifecycle: 2,
                    })
                    .collect::<Vec<_>>();
                let mut inputs = vec![&viewport.base as *const Base];
                inputs.extend(tags.iter_mut().map(|t| &t.base as *const Base));
                unsafe {
                    check((self.api.evaluate)(
                        0,
                        token,
                        inputs.as_ptr(),
                        inputs.len() as u32,
                        cmd,
                    ))
                }
            });
        self.performance.cpu_evaluate_ms = started.elapsed().as_secs_f32() * 1000.0;
        if let Err(e) = result {
            if let Some((m, slot)) = timing {
                m.cancel(slot);
            }
            return Err(e);
        }
        self.performance.completed_frames += 1;
        self.performance.reset_frames += u64::from(frame.reset);
        if let Some(m) = &self.monitor {
            self.performance.gpu_ms = m.gpu_ms;
            self.performance.gpu_samples = m.samples;
        }
        Ok(targets.output.create_view(&Default::default()))
    }
}

impl Drop for SuperResolution {
    fn drop(&mut self) {
        // Native descriptor image views are invisible to wgpu's usage tracker.
        // Keep them alive until all submitted DLSS work has completed.
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sdk_layouts_match_msvc() {
        use std::mem::size_of;
        assert_eq!(size_of::<sl::Options>(), 88);
        assert_eq!(size_of::<sl::Optimal>(), 64);
        assert_eq!(size_of::<sl::Constants>(), 456);
        assert_eq!(size_of::<sl::Resource>(), 112);
        assert_eq!(size_of::<sl::Tag>(), 64);
        assert_eq!(size_of::<sl::Viewport>(), 40);
        assert_eq!(size_of::<crate::abi::FeatureVersion>(), 56);
    }
    #[test]
    #[ignore = "requires the official SDK and an RTX Vulkan device"]
    fn real_dlss_evaluates_frames_and_returns_pixels() {
        let path = std::env::var_os("HORIZON_STREAMLINE_SDK").expect("SDK");
        let mut runtime = Runtime::load(
            std::path::Path::new(&path),
            "00a6bdb2-6386-4e37-8ff9-dcda4642628a",
        )
        .unwrap();
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        let instance = runtime.create_instance(&desc).unwrap();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        runtime.check_adapter(&adapter).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: adapter.features()
                & (wgpu::Features::TIMESTAMP_QUERY
                    | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS),
            ..Default::default()
        }))
        .unwrap();
        runtime.attach_device(&device).unwrap();
        let versions = runtime.dlss_versions().unwrap();
        println!("{versions}");
        assert!(!versions.contains("DLSS 0.0.0"));
        let runtime = Arc::new(Mutex::new(runtime));
        let mut sr = SuperResolution::new(runtime.clone(), &device, &queue).unwrap();
        for (label, preset) in [("Auto", 0), ("K", 11)] {
            for output in [[640, 360], [1920, 1080], [2560, 1440], [3840, 2160]] {
                let table = sr.recommended_sizes(output, preset).unwrap();
                println!(
                    "preset={label} output={}x{} quality={:?} balanced={:?} performance={:?} ultra={:?}",
                    output[0], output[1], table[0], table[1], table[2], table[3]
                );
                let optimal = [table[0][0], table[1][0], table[2][0], table[3][0]];
                assert!(
                    optimal[0][0] > optimal[1][0]
                        && optimal[1][0] > optimal[2][0]
                        && optimal[2][0] > optimal[3][0],
                    "DLSS modes collapsed to one render size: {optimal:?}"
                );
            }
        }
        for mode in [3, 2, 1, 4] {
            let size = sr.configure_with_preset(mode, [1920, 1080], 0).unwrap();
            println!(
                "configured mode={mode} render={}x{} output=1920x1080",
                size[0], size[1]
            );
        }
        sr.set_monitoring(true);
        let size = sr.configure(3, [640, 360]).unwrap();
        let make = |format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
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
                .create_view(&Default::default())
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let color = make(wgpu::TextureFormat::Rgba8Unorm, usage);
        let depth = make(wgpu::TextureFormat::Depth32Float, usage);
        let motion = make(wgpu::TextureFormat::Rg16Float, usage);
        let mut encoder = device.create_command_encoder(&Default::default());
        for (view, clear) in [
            (
                &color,
                wgpu::Color {
                    r: 0.25,
                    g: 0.5,
                    b: 0.75,
                    a: 1.,
                },
            ),
            (&motion, wgpu::Color::TRANSPARENT),
        ] {
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            }));
        }
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
        queue.submit([encoder.finish()]);
        let id = [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ];
        for i in 0..3 {
            let frame = FrameConstants {
                projection: id,
                inverse_projection: id,
                reprojection: id,
                inverse_reprojection: id,
                jitter_pixels: [0.; 2],
                camera: [[0.; 3], [0., 1., 0.], [1., 0., 0.], [0., 0., -1.]],
                near: 0.05,
                far: 1000.,
                fov: 1.2,
                aspect: 640. / 360.,
                reset: i == 0,
            };
            {
                let mut owner = runtime.lock().unwrap();
                owner.begin_render_frame();
                let token = owner.common_constants(frame).unwrap();
                owner.check_null_frame_tags(token);
            }
            let output = sr.evaluate(&color, &depth, &motion, frame).unwrap();
            // FG requests the same snapshot after SR; this exercises the real
            // SDK-backed common path without claiming generated Present.
            let mut owner = runtime.lock().unwrap();
            let token = owner.common_constants(frame).unwrap();
            assert!(!token.is_null());
            assert_eq!(owner.common_constants(frame).unwrap(), token);
            owner.check_null_frame_tags(token);
            owner.end_render_frame(true);
            drop(owner);
            println!("timing {:?}", sr.performance());
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: output.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 320,
                        y: 180,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let pixels = read.slice(..).get_mapped_range().unwrap();
            println!("Frame {i}: {:?}", &pixels[..8]);
            for (n, expected) in [0x3400u16, 0x3800, 0x3a00, 0x3c00].into_iter().enumerate() {
                let actual = u16::from_le_bytes([pixels[n * 2], pixels[n * 2 + 1]]);
                assert!(
                    actual.abs_diff(expected) < 32,
                    "Unexpected DLSS pixel: {actual:x}"
                );
            }
        }
        drop(sr);
        drop(runtime);
    }

    #[test]
    #[ignore = "requires the official SDK and an RTX DirectX 12 device"]
    fn real_dlss_evaluates_a_directx12_frame() {
        let path = std::env::var_os("HORIZON_STREAMLINE_SDK").expect("SDK");
        let mut runtime = Runtime::load_for(
            std::path::Path::new(&path),
            "00a6bdb2-6386-4e37-8ff9-dcda4642628a",
            crate::Api::Dx12,
        )
        .unwrap();
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        let instance = runtime.create_instance(&desc).unwrap();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        println!("Adapter: {}", adapter.get_info().name);
        runtime.check_adapter(&adapter).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        runtime.attach_device(&device).unwrap();
        let versions = runtime.dlss_versions().unwrap();
        println!("{versions}");
        assert!(!versions.contains("DLSS 0.0.0"));
        let runtime = Arc::new(Mutex::new(runtime));
        let mut sr = SuperResolution::new(runtime.clone(), &device, &queue).unwrap();
        let size = sr.configure(3, [640, 360]).unwrap();
        let make = |format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
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
                .create_view(&Default::default())
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let color = make(wgpu::TextureFormat::Rgba8Unorm, usage);
        let depth = make(wgpu::TextureFormat::Depth32Float, usage);
        let motion = make(wgpu::TextureFormat::Rg16Float, usage);
        let mut encoder = device.create_command_encoder(&Default::default());
        for (view, clear) in [
            (
                &color,
                wgpu::Color {
                    r: 0.25,
                    g: 0.5,
                    b: 0.75,
                    a: 1.,
                },
            ),
            (&motion, wgpu::Color::TRANSPARENT),
        ] {
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            }));
        }
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
        queue.submit([encoder.finish()]);
        let id = [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ];
        for i in 0..3 {
            let frame = FrameConstants {
                projection: id,
                inverse_projection: id,
                reprojection: id,
                inverse_reprojection: id,
                jitter_pixels: [0.; 2],
                camera: [[0.; 3], [0., 1., 0.], [1., 0., 0.], [0., 0., -1.]],
                near: 0.05,
                far: 1000.,
                fov: 1.2,
                aspect: 640. / 360.,
                reset: i == 0,
            };
            {
                let mut owner = runtime.lock().unwrap();
                owner.begin_render_frame();
                let token = owner.common_constants(frame).unwrap();
                owner.check_null_frame_tags(token);
            }
            let output = sr.evaluate(&color, &depth, &motion, frame).unwrap();
            let mut owner = runtime.lock().unwrap();
            let token = owner.common_constants(frame).unwrap();
            assert!(!token.is_null());
            assert_eq!(owner.common_constants(frame).unwrap(), token);
            owner.check_null_frame_tags(token);
            owner.end_render_frame(true);
            drop(owner);
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: output.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 320,
                        y: 180,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let pixels = read.slice(..).get_mapped_range().unwrap();
            println!("DirectX 12 frame {i}: {:?}", &pixels[..8]);
            for (n, expected) in [0x3400u16, 0x3800, 0x3a00, 0x3c00].into_iter().enumerate() {
                let actual = u16::from_le_bytes([pixels[n * 2], pixels[n * 2 + 1]]);
                assert!(
                    actual.abs_diff(expected) < 32,
                    "Unexpected DLSS pixel: {actual:x}"
                );
            }
        }
        drop(sr);
        drop(runtime);
    }
}
