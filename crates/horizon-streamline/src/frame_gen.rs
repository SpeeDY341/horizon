/*
* Copyright (c) 2022-2023 NVIDIA CORPORATION. All rights reserved
*
* Permission is hereby granted, free of charge, to any person obtaining a copy
* of this software and associated documentation files (the "Software"), to deal
* in the Software without restriction, including without limitation the rights
* to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
* copies of the Software, and to permit persons to whom the Software is
* furnished to do so, subject to the following conditions:
*
* The above copyright notice and this permission notice shall be included in all
* copies or substantial portions of the Software.
*
* THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
* IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
* FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
* AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
* LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
* OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
* SOFTWARE.
*/
// Rust ABI translation of Streamline 2.14.16 include/sl_dlss_g.h.
// Pinned source: NVIDIA-RTX/Streamline 625db07d4bca5abc67d781c42af7e7fb0761f1b6.
//! DLSS Frame Generation for one presented frame. 2× is one generated frame
//! (`numFramesToGenerate = 1`). 3×, 4× and 6× are the same call clamped to
//! `numFramesToGenerateMax`. The window thread owns every call. The frame
//! token is the one Reflex already opened for this present.
//!
//! The backbuffer is the one Present already shows. Depth and motion are
//! copied on a fresh command list submitted immediately after the list that
//! wrote that backbuffer (`eOnlyValidNow`). wgpu panics if raw tags are
//! recorded into a list that already drew. The hand pass clears the screen
//! depth before Present. The HUD is still in that color, so generated frames
//! smear it until a HUD-less tag exists.
use crate::{
    abi::Base,
    dlss_abi::{self as sl, GetFunction},
    evaluate::tagged_resource,
    super_resolution::FrameConstants,
};
use ash::vk::{self, Handle};
use std::{
    ffi::{CStr, c_void},
    time::{Duration, Instant},
};

const FEATURE: u32 = 1000;
const MODE_OFF: u32 = 0;
const MODE_ON: u32 = 1;
const RETAIN: u32 = 8;
const DEPTH: u32 = 0;
const MOTION: u32 = 1;
/// Copy during this command list. The textures are not intact at Present.
const ONLY_NOW: u32 = 0;

type SetOptions = unsafe extern "C" fn(*const sl::Viewport, *const GOptions) -> i32;
type GetState = unsafe extern "C" fn(*const sl::Viewport, *mut GState, *const GOptions) -> i32;
type SetTag = unsafe extern "C" fn(
    *const c_void,
    *const sl::Viewport,
    *const sl::Tag,
    u32,
    *mut c_void,
) -> i32;

/// `DLSSGOptions` v5. Pointer alignment inserts the pad before the callback.
#[repr(C)]
struct GOptions {
    base: Base,
    mode: u32,
    generated: u32,
    flags: u32,
    dynamic_width: u32,
    dynamic_height: u32,
    back_buffers: u32,
    mvec_width: u32,
    mvec_height: u32,
    color_width: u32,
    color_height: u32,
    color_format: u32,
    mvec_format: u32,
    depth_format: u32,
    hudless_format: u32,
    ui_format: u32,
    on_error: *const c_void,
    reserved15: u8,
    queue_parallelism: u32,
    ui_recomposition: u8,
    dynamic_fps: f32,
}

/// `DLSSGState` v4.
#[repr(C)]
struct GState {
    base: Base,
    vram: u64,
    status: u32,
    min_side: u32,
    presented: u32,
    max_generated: u32,
    reserved4: u8,
    vsync: u8,
    fence: *mut c_void,
    fence_value: u64,
    dynamic_mfg: u8,
}

struct Api {
    options: SetOptions,
    state: GetState,
    tag: SetTag,
}

pub struct Inputs<'a> {
    pub depth: Option<(&'a wgpu::Texture, &'a wgpu::TextureView)>,
    pub motion: Option<(&'a wgpu::Texture, &'a wgpu::TextureView)>,
    pub constants: Option<FrameConstants>,
    pub color: [u32; 2],
    pub depth_size: [u32; 2],
    pub color_format: wgpu::TextureFormat,
    /// False for a menu, HDR, a lost window, a resize, or a missing input.
    /// That present stays a real frame and keeps the plugin's resources.
    pub allow: bool,
}

pub(crate) struct Session {
    requested: bool,
    plugin_ready: bool,
    loaded: bool,
    api: Option<Api>,
    /// Generated frames the player asked for. Zero is off.
    wanted: u32,
    /// Zero until the driver confirms a nonzero upper bound.
    max_generated: u32,
    actual: u32,
    paused: bool,
    status: u32,
    rate: PresentRate,
    second_at: Instant,
    last_color: Option<[u32; 2]>,
    error: Option<String>,
    fault: Option<String>,
    reported: Option<String>,
    min_side: u32,
    needs_reset: bool,
    off_confirmed: bool,
}

impl Session {
    pub(crate) fn new(requested: bool, plugin_ready: bool) -> Self {
        Self {
            requested,
            plugin_ready,
            loaded: false,
            api: None,
            wanted: 0,
            max_generated: 0,
            actual: 0,
            paused: true,
            status: 0,
            rate: PresentRate::default(),
            second_at: Instant::now(),
            last_color: None,
            error: None,
            fault: None,
            reported: None,
            min_side: 0,
            needs_reset: true,
            off_confirmed: false,
        }
    }

    pub(crate) fn requested(&self) -> bool {
        self.requested
    }

    pub(crate) fn loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn reset_pending(&self) -> bool {
        self.needs_reset
    }

    /// Keep the lifecycle request pending while generation is blocked. Only
    /// an attempt that can reach native On may reset shared SR/TAA history.
    pub(crate) fn reset_ready(
        &self,
        output: [u32; 2],
        reflex_active: bool,
        presentation_ready: bool,
    ) -> bool {
        self.loaded
            && self.wanted > 0
            && self.max_generated > 0
            && self.fault.is_none()
            && self.dimensions_supported(output)
            && reflex_active
            && presentation_ready
            && self.reset_pending()
    }

    fn dimensions_supported(&self, output: [u32; 2]) -> bool {
        output.into_iter().all(|side| side >= self.min_side)
    }

    pub(crate) fn invalidate_history(&mut self) {
        self.needs_reset = true;
    }

    fn frame_ready(&self, allow: bool, inputs_ready: bool, reset: bool) -> bool {
        allow
            && inputs_ready
            && self.wanted > 0
            && self.max_generated > 0
            && self.fault.is_none()
            && (!self.needs_reset || reset)
    }

    pub(crate) fn set_wanted(&mut self, generated: u32) {
        if self.wanted != generated {
            self.needs_reset = true;
            if self.loaded {
                self.fault = None; // Explicit mode change retries a failed SDK frame.
            }
            self.rate = PresentRate::default();
            self.second_at = Instant::now();
        }
        self.wanted = generated;
        if generated == 0 {
            self.actual = 0;
            self.paused = false;
        }
    }

    pub(crate) fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    pub(crate) fn fail(&mut self, error: String) {
        self.loaded = false;
        self.off_confirmed = false;
        self.note(error);
    }

    pub(crate) fn max_generated(&self) -> u32 {
        if self.loaded && self.fault.is_none() {
            self.max_generated
        } else {
            0
        }
    }

    /// Must succeed before touching a swapchain or releasing its inputs. Do
    /// not use `loaded` here: a failed frame may still have left the SDK On.
    pub(crate) fn suspend(&mut self) -> Result<(), String> {
        self.needs_reset = true;
        self.last_color = None;
        self.actual = 0;
        self.paused = self.wanted > 0;
        if let Some(api) = &self.api
            && !self.off_confirmed
        {
            // Sizes/formats are optional in the pinned v5 ABI. No frame token
            // or live input texture is needed to turn generation off.
            let options = GOptions::new(MODE_OFF, 1, [0; 2], [0; 2], 0, 0, 0);
            let viewport = sl::Viewport::new();
            // Safety: called on the present thread, with live v5 ABI values.
            let code = unsafe { (api.options)(&viewport, &options) };
            if code != 0 {
                return Err(format!("Frame Generation suspend failed ({code})"));
            }
            self.off_confirmed = true;
        }
        Ok(())
    }

    /// Bind after the device exists. A missing plugin leaves the device usable.
    pub(crate) fn bind(
        &mut self,
        library: &libloading::os::windows::Library,
    ) -> Result<(), String> {
        if !self.requested || !self.plugin_ready || self.api.is_some() {
            return Ok(());
        }
        // Safety: the library outlives the pointers. Names are the pinned ABI.
        let api = unsafe {
            let get = *library
                .get::<GetFunction>(b"slGetFeatureFunction\0")
                .map_err(|e| e.to_string())?;
            let function = |name: &CStr| -> Result<*mut c_void, String> {
                let mut ptr = std::ptr::null_mut();
                let code = get(FEATURE, name.as_ptr(), &mut ptr);
                if code != 0 || ptr.is_null() {
                    return Err(format!(
                        "Frame Generation function {} missing ({code})",
                        name.to_string_lossy()
                    ));
                }
                Ok(ptr)
            };
            Api {
                options: std::mem::transmute::<*mut c_void, SetOptions>(function(
                    c"slDLSSGSetOptions",
                )?),
                state: std::mem::transmute::<*mut c_void, GetState>(function(c"slDLSSGGetState")?),
                tag: *library
                    .get::<SetTag>(b"slSetTagForFrame\0")
                    .map_err(|e| e.to_string())?,
            }
        };
        self.api = Some(api);
        self.loaded = true;
        Ok(())
    }

    /// Constants, tags and options for the present that follows this submit.
    /// `token` is Reflex's frame token. A null token cannot mark the frame.
    /// Barriers stay on `encoder`. The returned list, when present, holds only
    /// the raw tag copy and must be submitted after `encoder` and before Present.
    pub(crate) fn record(
        &mut self,
        dx12: bool,
        token: *mut c_void,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        input: Inputs<'_>,
    ) -> Result<Option<wgpu::CommandBuffer>, String> {
        if !self.loaded {
            return Ok(None);
        }
        let resized = self.last_color.is_some_and(|prev| prev != input.color);
        self.last_color = Some(input.color);
        let generate = if self.wanted == 0 {
            1
        } else {
            self.wanted.min(self.max_generated)
        };
        let inputs_ready = !resized
            && self.dimensions_supported(input.color)
            && input.depth.is_some()
            && input.motion.is_some()
            && input.constants.is_some()
            && !token.is_null();
        let interpolate = self.frame_ready(
            input.allow,
            inputs_ready,
            input.constants.as_ref().is_some_and(|c| c.reset),
        );
        self.actual = if interpolate { generate } else { 0 };
        self.paused = self.wanted > 0 && !interpolate;
        if interpolate && let Err(error) = self.transition(encoder, &input) {
            return self.fallback(dx12, token, device, error);
        }
        let tags = if token.is_null() {
            None
        } else {
            match self.record_tags(dx12, token, device, interpolate, &input) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    return self.fallback(dx12, token, device, error);
                }
            }
        };
        if let Err(error) = self.apply(dx12, interpolate, generate.max(1), &input) {
            drop(tags);
            return self.fallback(dx12, token, device, error);
        }
        Ok(tags)
    }

    fn fallback(
        &mut self,
        dx12: bool,
        token: *mut c_void,
        device: &wgpu::Device,
        error: String,
    ) -> Result<Option<wgpu::CommandBuffer>, String> {
        self.note(error);
        self.suspend()?;
        // Replace any live frame tags installed before failure, on the same
        // token. Only the cleared list is allowed to reach the submit.
        if token.is_null() {
            Ok(None)
        } else {
            self.record_tags(
                dx12,
                token,
                device,
                false,
                &Inputs {
                    depth: None,
                    motion: None,
                    constants: None,
                    color: [0; 2],
                    depth_size: [0; 2],
                    color_format: wgpu::TextureFormat::Rgba8Unorm,
                    allow: false,
                },
            )
            .map(Some)
        }
    }

    /// `numFramesActuallyPresented` since the previous call, on this thread only.
    pub(crate) fn sample(&mut self) {
        let Some(api) = &self.api else {
            return;
        };
        if !self.loaded {
            return;
        }
        let viewport = sl::Viewport::new();
        let mut state = GState::empty();
        // Safety: not thread-safe, and only the window thread reaches this.
        // The state struct is the writable v4 ABI.
        let code = unsafe { (api.state)(&viewport, &mut state, std::ptr::null()) };
        if code != 0 {
            self.max_generated = 0;
            self.note(format!("Frame Generation state failed ({code})"));
            if let Err(error) = self.suspend() {
                self.note(error);
            }
            return;
        }
        self.max_generated = state.max_generated;
        self.min_side = state.min_side;
        let now = Instant::now();
        if self
            .rate
            .sample(state.presented, now.duration_since(self.second_at))
        {
            self.second_at = now;
        }
        self.status = state.status;
        if self.wanted > 0 && state.status != 0 {
            self.note(format!("Frame Generation status {:#x}", state.status));
            if let Err(error) = self.suspend() {
                self.note(error);
            }
        } else if self.wanted > 0 && state.max_generated == 0 {
            if let Err(error) = self.suspend() {
                self.note(error);
            }
        }
    }

    pub(crate) fn line(&self) -> String {
        if let Some(error) = &self.fault {
            let retry = if self.loaded {
                "; change mode to retry"
            } else {
                ""
            };
            return format!("Frame Gen: unavailable ({error}{retry})");
        }
        if !self.loaded {
            return String::new();
        }
        if self.wanted == 0 {
            return "Frame Gen: off".into();
        }
        if self.max_generated == 0 {
            return "Frame Gen: paused (waiting for driver capabilities)".into();
        }
        if self.paused || self.status != 0 || self.actual == 0 {
            return "Frame Gen: paused".into();
        }
        format!(
            "Frame Gen: {}x configured ({}x requested) · {:.0} SDK shown FPS · {:.0} rendered FPS",
            self.actual + 1,
            self.wanted + 1,
            self.rate.shown,
            self.rate.rendered
        )
    }

    pub(crate) fn note(&mut self, error: String) {
        if self.reported.as_ref() != Some(&error) {
            self.error = Some(error.clone());
            self.reported = Some(error.clone());
        }
        self.fault = Some(error);
        self.actual = 0;
        self.paused = self.wanted > 0;
        self.needs_reset = true;
    }

    fn apply(
        &mut self,
        dx12: bool,
        on: bool,
        generate: u32,
        input: &Inputs<'_>,
    ) -> Result<(), String> {
        let Some(api) = &self.api else {
            return Ok(());
        };
        if !on && self.off_confirmed {
            return Ok(());
        }
        let options = GOptions::new(
            if on { MODE_ON } else { MODE_OFF },
            generate,
            input.color,
            input.depth_size,
            color_format(input.color_format, dx12),
            if dx12 {
                34
            } else {
                vk::Format::R16G16_SFLOAT.as_raw() as u32
            },
            if dx12 {
                40
            } else {
                vk::Format::D32_SFLOAT.as_raw() as u32
            },
        );
        let viewport = sl::Viewport::new();
        // Safety: options and the viewport outlive the call. The callback is null.
        let code = unsafe { (api.options)(&viewport, &options) };
        self.off_confirmed = code == 0 && !on;
        if code != 0 {
            self.actual = 0;
            self.paused = self.wanted > 0;
            return Err(format!("Frame Generation options failed ({code})"));
        }
        if on {
            self.needs_reset = false;
        } else if self.wanted > 0 {
            self.needs_reset = true;
        }
        Ok(())
    }

    /// Depth and motion must be in the resource state before the tag list runs.
    /// This barrier stays on the drawn list. The tag list itself cannot take it:
    /// wgpu rejects a barrier and a raw command on the same encoder.
    fn transition(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: &Inputs<'_>,
    ) -> Result<(), String> {
        let (Some((depth, _)), Some((motion, _))) = (input.depth, input.motion) else {
            return Err("Frame Generation is missing depth or motion".into());
        };
        encoder.transition_resources(
            std::iter::empty(),
            [depth, motion]
                .into_iter()
                .map(|texture| wgpu::TextureTransition {
                    texture,
                    selector: None,
                    state: wgpu::TextureUses::RESOURCE,
                }),
        );
        Ok(())
    }

    /// Tag on an encoder that has no wgpu commands yet. Submit that buffer
    /// after the drawn frame and before Present.
    fn record_tags(
        &mut self,
        dx12: bool,
        token: *mut c_void,
        device: &wgpu::Device,
        interpolate: bool,
        input: &Inputs<'_>,
    ) -> Result<wgpu::CommandBuffer, String> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Frame Generation"),
        });
        let tagged = if interpolate {
            self.tag(dx12, token, &mut encoder, input)
        } else {
            self.tag_cleared(dx12, token, &mut encoder)
        };
        match tagged {
            Ok(()) => Ok(encoder.finish()),
            Err(error) => {
                drop(encoder.finish());
                Err(error)
            }
        }
    }

    fn tag(
        &mut self,
        dx12: bool,
        token: *mut c_void,
        encoder: &mut wgpu::CommandEncoder,
        input: &Inputs<'_>,
    ) -> Result<(), String> {
        let (Some((depth, depth_view)), Some((motion, motion_view))) = (input.depth, input.motion)
        else {
            return Err("Frame Generation is missing depth or motion".into());
        };
        if dx12 {
            self.tag_dx12(token, encoder, depth, motion, input.depth_size)
        } else {
            self.tag_vulkan(
                token,
                encoder,
                depth,
                depth_view,
                motion,
                motion_view,
                input.depth_size,
            )
        }
    }

    fn tag_cleared(
        &mut self,
        dx12: bool,
        token: *mut c_void,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), String> {
        let tags = [null_tag(DEPTH), null_tag(MOTION)];
        self.send_tags(dx12, token, encoder, &tags)
    }

    fn tag_vulkan(
        &mut self,
        token: *mut c_void,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
        depth_view: &wgpu::TextureView,
        motion: &wgpu::Texture,
        motion_view: &wgpu::TextureView,
        size: [u32; 2],
    ) -> Result<(), String> {
        // Safety: both textures belong to this device and outlive the submit.
        // The views are the ones stored beside them, so the copy can run at submit.
        let mut resources = unsafe {
            let depth_image = depth
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("Vulkan depth missing")?
                .raw_handle();
            let motion_image = motion
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("Vulkan motion missing")?
                .raw_handle();
            let depth_raw = depth_view
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("Vulkan depth view missing")?
                .raw_handle();
            let motion_raw = motion_view
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or("Vulkan motion view missing")?
                .raw_handle();
            let depth_usage = (vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC)
                .as_raw();
            let motion_usage = (vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC)
                .as_raw();
            [
                tagged_resource(
                    depth_image.as_raw() as *mut c_void,
                    depth_raw.as_raw() as *mut c_void,
                    vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL.as_raw() as u32,
                    depth.width(),
                    depth.height(),
                    vk::Format::D32_SFLOAT.as_raw() as u32,
                    depth_usage,
                ),
                tagged_resource(
                    motion_image.as_raw() as *mut c_void,
                    motion_raw.as_raw() as *mut c_void,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL.as_raw() as u32,
                    motion.width(),
                    motion.height(),
                    vk::Format::R16G16_SFLOAT.as_raw() as u32,
                    motion_usage,
                ),
            ]
        };
        let tags = [
            live_tag(&mut resources[0], DEPTH, size),
            live_tag(&mut resources[1], MOTION, size),
        ];
        self.send_tags(false, token, encoder, &tags)
    }

    fn tag_dx12(
        &mut self,
        token: *mut c_void,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
        motion: &wgpu::Texture,
        size: [u32; 2],
    ) -> Result<(), String> {
        const SHADER_RESOURCE: u32 = 0x40 | 0x80;
        const D32_FLOAT: u32 = 40;
        const R16G16_FLOAT: u32 = 34;
        // Safety: textures belong to this device. A null view is valid on Direct3D.
        let mut resources = unsafe {
            let depth_image = depth
                .as_hal::<wgpu::hal::api::Dx12>()
                .ok_or("DirectX 12 depth missing")?
                .raw_resource_ptr();
            let motion_image = motion
                .as_hal::<wgpu::hal::api::Dx12>()
                .ok_or("DirectX 12 motion missing")?
                .raw_resource_ptr();
            if depth_image.is_null() || motion_image.is_null() {
                return Err("DirectX 12 Frame Generation texture missing".into());
            }
            [
                tagged_resource(
                    depth_image,
                    std::ptr::null_mut(),
                    SHADER_RESOURCE,
                    depth.width(),
                    depth.height(),
                    D32_FLOAT,
                    0,
                ),
                tagged_resource(
                    motion_image,
                    std::ptr::null_mut(),
                    SHADER_RESOURCE,
                    motion.width(),
                    motion.height(),
                    R16G16_FLOAT,
                    0,
                ),
            ]
        };
        let tags = [
            live_tag(&mut resources[0], DEPTH, size),
            live_tag(&mut resources[1], MOTION, size),
        ];
        self.send_tags(true, token, encoder, &tags)
    }

    fn send_tags(
        &mut self,
        dx12: bool,
        token: *mut c_void,
        encoder: &mut wgpu::CommandEncoder,
        tags: &[sl::Tag],
    ) -> Result<(), String> {
        let Some(api) = &self.api else {
            return Ok(());
        };
        let tag = api.tag;
        let viewport = sl::Viewport::new();
        // Safety: the command list stays open for wgpu to close. Tags point at
        // resources that outlive this call, or at nothing when the frame is paused.
        if dx12 {
            unsafe {
                encoder.as_hal_mut::<wgpu::hal::api::Dx12, _, _>(|hal| -> Result<(), String> {
                    let hal = hal.ok_or("DirectX 12 command encoder missing")?;
                    let list = hal
                        .raw_open_command_list_ptr()
                        .ok_or("DirectX 12 command list is not open")?;
                    let code = tag(token, &viewport, tags.as_ptr(), tags.len() as u32, list);
                    if code == 0 {
                        Ok(())
                    } else {
                        Err(format!("Frame Generation tag failed ({code})"))
                    }
                })?;
            }
        } else {
            unsafe {
                encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(
                    |hal| -> Result<(), String> {
                        let hal = hal.ok_or("Vulkan command encoder missing")?;
                        let code = tag(
                            token,
                            &viewport,
                            tags.as_ptr(),
                            tags.len() as u32,
                            hal.raw_handle().as_raw() as *mut c_void,
                        );
                        if code == 0 {
                            Ok(())
                        } else {
                            Err(format!("Frame Generation tag failed ({code})"))
                        }
                    },
                )?;
            }
        }
        Ok(())
    }
}

/// SDK presents and application presents measured over the same real interval.
/// The requested generation multiplier never participates in this measurement.
#[derive(Default)]
struct PresentRate {
    presented: u64,
    frames: u64,
    shown: f64,
    rendered: f64,
}
impl PresentRate {
    fn sample(&mut self, presented: u32, elapsed: Duration) -> bool {
        self.presented = self.presented.saturating_add(u64::from(presented));
        self.frames = self.frames.saturating_add(1);
        if elapsed < Duration::from_secs(1) {
            return false;
        }
        self.shown = self.presented as f64 / elapsed.as_secs_f64();
        self.rendered = self.frames as f64 / elapsed.as_secs_f64();
        self.presented = 0;
        self.frames = 0;
        true
    }
}

impl GOptions {
    fn new(
        mode: u32,
        generated: u32,
        color: [u32; 2],
        depth: [u32; 2],
        color_format: u32,
        mvec_format: u32,
        depth_format: u32,
    ) -> Self {
        Self {
            base: Base::new(
                0xfac5f1cb,
                0x2dfd,
                0x4f36,
                [0xa1, 0xe6, 0x3a, 0x9e, 0x86, 0x52, 0x56, 0xc5],
                5,
            ),
            mode,
            generated,
            flags: RETAIN,
            dynamic_width: 0,
            dynamic_height: 0,
            back_buffers: 0,
            mvec_width: depth[0],
            mvec_height: depth[1],
            color_width: color[0],
            color_height: color[1],
            color_format,
            mvec_format,
            depth_format,
            hudless_format: 0,
            ui_format: 0,
            on_error: std::ptr::null(),
            // Boolean::eInvalid. The field is reserved.
            reserved15: 2,
            queue_parallelism: 0,
            ui_recomposition: 0,
            dynamic_fps: 0.0,
        }
    }
}

impl GState {
    fn empty() -> Self {
        Self {
            base: Base::new(
                0xcc8ac8e1,
                0xa179,
                0x44f5,
                [0x97, 0xfa, 0xe7, 0x41, 0x12, 0xf9, 0xbc, 0x61],
                4,
            ),
            vram: 0,
            status: 0,
            min_side: 0,
            presented: 0,
            max_generated: 0,
            reserved4: 0,
            vsync: 0,
            fence: std::ptr::null_mut(),
            fence_value: 0,
            dynamic_mfg: 0,
        }
    }
}

fn live_tag(resource: &mut sl::Resource, kind: u32, size: [u32; 2]) -> sl::Tag {
    sl::Tag {
        base: tag_base(),
        resource,
        kind,
        lifecycle: ONLY_NOW,
        extent: [0, 0, size[0], size[1]],
    }
}

pub(crate) fn null_tag(kind: u32) -> sl::Tag {
    sl::Tag {
        base: tag_base(),
        resource: std::ptr::null_mut(),
        kind,
        lifecycle: ONLY_NOW,
        extent: [0; 4],
    }
}

fn tag_base() -> Base {
    Base::new(
        0x4c6a5aad,
        0xb445,
        0x496c,
        [0x87, 0xff, 0x1a, 0xf3, 0x84, 0x5b, 0xe6, 0x53],
        1,
    )
}

fn color_format(format: wgpu::TextureFormat, dx12: bool) -> u32 {
    use wgpu::TextureFormat::*;
    if dx12 {
        // The swapchain resource is UNORM. sRGB is only the render-target view.
        match format {
            Bgra8Unorm | Bgra8UnormSrgb => 87,
            Rgba8Unorm | Rgba8UnormSrgb => 28,
            Rgba16Float => 10,
            _ => 0,
        }
    } else {
        match format {
            Bgra8Unorm => 44,
            Bgra8UnormSrgb => 50,
            Rgba8Unorm => 37,
            Rgba8UnormSrgb => 43,
            Rgba16Float => 97,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn present_rates_use_sdk_counts_and_elapsed_time_including_stalls() {
        let mut rate = PresentRate::default();
        assert!(!rate.sample(2, Duration::from_millis(100)));
        assert!(rate.sample(2, Duration::from_secs(2)));
        assert_eq!((rate.shown, rate.rendered), (2., 1.));
        // A selected 2x mode can still present only a real frame, or none.
        assert!(rate.sample(1, Duration::from_secs(1)));
        assert_eq!((rate.shown, rate.rendered), (1., 1.));
        assert!(rate.sample(0, Duration::from_secs(4)));
        assert_eq!((rate.shown, rate.rendered), (0., 0.25));
    }

    thread_local! {
        static OPTIONS: std::cell::RefCell<(Vec<u32>, i32)> = const { std::cell::RefCell::new((Vec::new(), 0)) };
        static STATE: std::cell::Cell<(u32, u32, u32, i32)> = const { std::cell::Cell::new((1, 0, 0, 0)) };
        static TAGS: std::cell::RefCell<Vec<bool>> = const { std::cell::RefCell::new(Vec::new()) };
        static FAIL: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((false, false)) };
        static GENERATED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    unsafe extern "C" fn options(_: *const sl::Viewport, options: *const GOptions) -> i32 {
        // Safety: Session passes a live options value during this call.
        let options = unsafe { &*options };
        assert_eq!(options.queue_parallelism, 0);
        GENERATED.set(options.generated);
        if options.mode == MODE_ON && FAIL.get().0 {
            return 7;
        }
        OPTIONS.with_borrow_mut(|(calls, result)| {
            calls.push(options.mode);
            *result
        })
    }
    unsafe extern "C" fn state(
        _: *const sl::Viewport,
        state: *mut GState,
        _: *const GOptions,
    ) -> i32 {
        let (cap, status, presented, code) = STATE.get();
        let state = unsafe { &mut *state };
        state.max_generated = cap;
        state.status = status;
        state.presented = presented;
        code
    }
    unsafe extern "C" fn tag(
        _: *const c_void,
        _: *const sl::Viewport,
        tags: *const sl::Tag,
        count: u32,
        _: *mut c_void,
    ) -> i32 {
        assert_eq!(count, 2);
        let live = unsafe { !(*tags).resource.is_null() };
        TAGS.with_borrow_mut(|calls| calls.push(live));
        if live && FAIL.get().1 { 9 } else { 0 }
    }

    #[test]
    fn suspend_calls_sdk_even_after_failure_and_never_clears_the_request() {
        let mut session = Session::new(true, true);
        session.api = Some(Api {
            options,
            state,
            tag,
        });
        session.wanted = 3;
        session.actual = 3;
        session.last_color = Some([1920, 1080]);
        session.needs_reset = false;
        // Failure disables recording, but is not proof that the SDK is Off.
        session.fail("frame failed".into());
        OPTIONS.with_borrow_mut(|state| *state = (Vec::new(), 7));
        assert!(session.suspend().is_err());
        assert!(session.needs_reset);
        assert_eq!(session.last_color, None);
        assert_eq!(session.actual, 0);
        assert_eq!(session.wanted, 3);
        OPTIONS.with_borrow_mut(|(_, result)| *result = 0);
        session.suspend().unwrap();
        OPTIONS.with_borrow(|(calls, _)| assert_eq!(calls, &[MODE_OFF, MODE_OFF]));
        assert!(session.paused);
    }

    #[test]
    fn pending_reset_survives_paused_frames_and_failed_resume() {
        let mut session = Session::new(true, true);
        session.api = Some(Api {
            options,
            state,
            tag,
        });
        session.wanted = 1;
        session.max_generated = 1;
        OPTIONS.with_borrow_mut(|state| *state = (Vec::new(), 0));
        session.suspend().unwrap();
        session.suspend().unwrap(); // already confirmed Off before any Present
        let input = Inputs {
            depth: None,
            motion: None,
            constants: None,
            color: [1280, 720],
            depth_size: [1280, 720],
            color_format: wgpu::TextureFormat::Rgba8Unorm,
            allow: false,
        };
        for reset in [true, false, false] {
            let on = session.frame_ready(false, true, reset);
            assert!(!on);
            session.apply(false, on, 1, &input).unwrap();
            assert!(session.reset_pending());
        }
        assert!(!session.frame_ready(true, true, false));
        // The renderer queries this before jitter/common constants, so the
        // eligible frame gets reset even after menu frames consumed theirs.
        let reset = session.reset_pending();
        let on = session.frame_ready(true, true, reset);
        assert!(on);
        OPTIONS.with_borrow_mut(|(_, result)| *result = 7);
        assert!(session.apply(false, on, 1, &input).is_err());
        assert!(session.reset_pending());
        OPTIONS.with_borrow_mut(|(_, result)| *result = 0);
        session.apply(false, on, 1, &input).unwrap();
        assert!(!session.reset_pending());
        assert!(session.frame_ready(true, true, false));
        session.apply(false, true, 1, &input).unwrap();
        OPTIONS.with_borrow(|(calls, _)| assert_eq!(calls, &[MODE_OFF, MODE_ON, MODE_ON, MODE_ON]));
    }

    #[test]
    fn pending_history_reset_waits_for_a_possible_generation_attempt() {
        let mut session = Session::new(true, true);
        session.loaded = true;
        session.set_wanted(1);
        session.max_generated = 1;
        assert!(session.reset_ready([64; 2], true, true));
        session.note("mock SDK frame failure".into());
        for _ in 0..4 {
            assert!(session.reset_pending());
            assert!(!session.reset_ready([64; 2], true, true));
        }
        session.set_wanted(0);
        session.set_wanted(1);
        session.max_generated = 0;
        assert!(session.reset_pending());
        assert!(!session.reset_ready([64; 2], true, true));
        session.max_generated = 1;
        assert!(!session.reset_ready([64; 2], false, true));
        assert!(!session.reset_ready([64; 2], true, false));
        assert!(session.reset_pending());
        session.min_side = 32;
        for output in [[31, 64], [64, 31], [31, 31]] {
            assert!(!session.dimensions_supported(output));
            assert!(!session.reset_ready(output, true, true));
            assert!(session.reset_pending());
        }
        assert!(session.dimensions_supported([32; 2]));
        assert!(session.reset_ready([32; 2], true, true));
        assert!(session.reset_ready([64; 2], true, true));
        // Native On success consumes the pending request, not these queries.
        session.needs_reset = false;
        assert!(!session.reset_ready([64; 2], true, true));
    }

    #[test]
    fn sdk_state_errors_stop_generation_until_explicit_retry() {
        for (status, code) in [(8, 0), (0, 42)] {
            let mut session = Session::new(true, true);
            session.api = Some(Api {
                options,
                state,
                tag,
            });
            session.loaded = true;
            session.set_wanted(5);
            OPTIONS.with_borrow_mut(|state| *state = (Vec::new(), 0));
            STATE.set((5, status, 0, code));
            session.sample();
            assert!(!session.frame_ready(true, true, true));
            assert!(session.off_confirmed);
            assert_eq!(session.actual, 0);
            assert_eq!(session.max_generated(), 0);
            assert!(session.line().contains("unavailable"));
            assert!(session.take_error().is_some());
            session.sample();
            assert!(
                session.take_error().is_none(),
                "repeated error must not log every frame"
            );
            STATE.set((1, 0, 0, 0));
            session.sample();
            assert!(!session.frame_ready(true, true, true));
            session.set_wanted(0);
            session.set_wanted(5);
            assert!(session.frame_ready(true, true, true));
            // A later unknown cap must discard the previous confirmed 6x.
            STATE.set((0, 0, 0, 0));
            session.sample();
            assert_eq!(session.max_generated(), 0);
            assert!(!session.frame_ready(true, true, true));
            assert!(session.line().contains("driver capabilities"));
        }
    }

    #[test]
    #[ignore = "requires Vulkan and DirectX 12 devices; SDK calls are mocked"]
    fn failed_live_tags_or_options_submit_only_cleared_tags_on_both_apis() {
        for backend in [wgpu::Backends::VULKAN, wgpu::Backends::DX12] {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: backend,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter =
                pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
            let (device, queue) =
                pollster::block_on(adapter.request_device(&Default::default())).unwrap();
            let texture = |format| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            };
            let depth = texture(wgpu::TextureFormat::Depth32Float);
            let motion = texture(wgpu::TextureFormat::Rg16Float);
            let depth_view = depth.create_view(&Default::default());
            let motion_view = motion.create_view(&Default::default());
            for failure in [(true, false), (false, true)] {
                let mut session = Session::new(true, true);
                session.api = Some(Api {
                    options,
                    state,
                    tag,
                });
                session.loaded = true;
                session.set_wanted(5);
                OPTIONS.with_borrow_mut(|state| *state = (Vec::new(), 0));
                STATE.set((1, 0, 0, 0));
                session.sample();
                let input = || Inputs {
                    depth: Some((&depth, &depth_view)),
                    motion: Some((&motion, &motion_view)),
                    constants: Some(FrameConstants {
                        projection: [[0.; 4]; 4],
                        inverse_projection: [[0.; 4]; 4],
                        reprojection: [[0.; 4]; 4],
                        inverse_reprojection: [[0.; 4]; 4],
                        jitter_pixels: [0.; 2],
                        camera: [[0.; 3]; 4],
                        near: 0.05,
                        far: 1000.,
                        fov: 1.2,
                        aspect: 1.,
                        reset: true,
                    }),
                    color: [64; 2],
                    depth_size: [64; 2],
                    color_format: wgpu::TextureFormat::Rgba8Unorm,
                    allow: true,
                };
                let record = |session: &mut Session, allow| {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    let mut input = input();
                    input.allow = allow;
                    let tags = session
                        .record(
                            backend == wgpu::Backends::DX12,
                            std::ptr::dangling_mut(),
                            &device,
                            &mut encoder,
                            input,
                        )
                        .unwrap();
                    queue.submit(std::iter::once(encoder.finish()).chain(tags));
                };
                TAGS.with_borrow_mut(Vec::clear);
                FAIL.set(failure);
                record(&mut session, true);
                TAGS.with_borrow(|calls| assert_eq!(calls, &[true, false]));
                assert!(session.off_confirmed);
                assert_eq!(session.actual, 0);
                assert!(session.reset_pending());
                FAIL.set((false, false));
                record(&mut session, true);
                assert_eq!(
                    session.actual, 0,
                    "no implicit retry after failed SDK frame"
                );
                session.set_wanted(0);
                session.set_wanted(5);
                record(&mut session, true);
                assert_eq!(
                    session.actual, 1,
                    "driver cap, not request, selects the options"
                );
                assert_eq!(GENERATED.get(), 1);
                session.second_at = Instant::now() - Duration::from_secs(2);
                session.sample();
                assert!(session.line().contains("2x configured (6x requested)"));
                assert!(session.line().contains("0 SDK shown FPS"));
                assert!(session.rate.rendered > 0.);
                // Renderer menu policy sets allow=false even when blur is
                // absent and valid world inputs are still available. Exercise
                // the actual recording/submission path, not only frame_ready.
                TAGS.with_borrow_mut(Vec::clear);
                OPTIONS.with_borrow_mut(|state| *state = (Vec::new(), 0));
                for _ in 0..3 {
                    record(&mut session, false);
                    assert_eq!(session.actual, 0);
                    assert!(session.off_confirmed);
                    assert!(session.reset_pending());
                    assert_eq!(session.wanted, 5);
                }
                TAGS.with_borrow(|calls| assert_eq!(calls, &[false, false, false]));
                OPTIONS.with_borrow(|(calls, _)| assert_eq!(calls, &[MODE_OFF]));
                record(&mut session, true);
                assert_eq!(session.actual, 1);
                assert!(!session.reset_pending());
                TAGS.with_borrow(|calls| assert_eq!(calls.last(), Some(&true)));
                OPTIONS.with_borrow(|(calls, _)| assert_eq!(calls, &[MODE_OFF, MODE_ON]));
                session.min_side = 128;
                record(&mut session, true);
                assert_eq!(session.actual, 0, "below driver minimum must pause");
                TAGS.with_borrow(|calls| assert_eq!(calls.last(), Some(&false)));
                session.suspend().unwrap();
            }
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        }
    }

    #[test]
    fn options_and_state_match_the_pinned_header() {
        assert_eq!(size_of::<GOptions>(), 120);
        assert_eq!(offset_of!(GOptions, mode), 32);
        assert_eq!(offset_of!(GOptions, generated), 36);
        assert_eq!(offset_of!(GOptions, color_format), 72);
        assert_eq!(offset_of!(GOptions, ui_format), 88);
        assert_eq!(offset_of!(GOptions, on_error), 96);
        assert_eq!(offset_of!(GOptions, reserved15), 104);
        assert_eq!(offset_of!(GOptions, queue_parallelism), 108);
        assert_eq!(offset_of!(GOptions, ui_recomposition), 112);
        assert_eq!(offset_of!(GOptions, dynamic_fps), 116);
        assert_eq!(size_of::<GState>(), 88);
        assert_eq!(offset_of!(GState, vram), 32);
        assert_eq!(offset_of!(GState, status), 40);
        assert_eq!(offset_of!(GState, presented), 48);
        assert_eq!(offset_of!(GState, max_generated), 52);
        assert_eq!(offset_of!(GState, reserved4), 56);
        assert_eq!(offset_of!(GState, vsync), 57);
        assert_eq!(offset_of!(GState, fence), 64);
        assert_eq!(offset_of!(GState, fence_value), 72);
        assert_eq!(offset_of!(GState, dynamic_mfg), 80);
    }
}
