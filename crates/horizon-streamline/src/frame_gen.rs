//! DLSS Frame Generation for one presented frame. 2× is one generated frame
//! (`numFramesToGenerate = 1`). 3×, 4× and 6× are the same call clamped to
//! `numFramesToGenerateMax`. The window thread owns every call. The frame
//! token is the one Reflex already opened for this present.
//!
//! The backbuffer is the one Present already shows. Depth and motion are
//! copied on the command list that writes that backbuffer (`eOnlyValidNow`),
//! because the hand pass clears the screen depth before Present. The HUD is
//! still in that color, so generated frames smear it until a HUD-less tag exists.
use crate::{
    abi::Base,
    dlss_abi::{self as sl, GetFunction},
    evaluate::tagged_resource,
    super_resolution::{self, FrameConstants},
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
    constants: sl::SetConstants,
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
    /// Driver cap. One until `GetState` says more, so 2× is the first guess.
    max_generated: u32,
    actual: u32,
    paused: bool,
    status: u32,
    presented_second: u32,
    presented_shown: u32,
    have_second: bool,
    second_at: Instant,
    last_color: Option<[u32; 2]>,
    error: Option<String>,
}

impl Session {
    pub(crate) fn new(requested: bool, plugin_ready: bool) -> Self {
        Self {
            requested,
            plugin_ready,
            loaded: false,
            api: None,
            wanted: 0,
            max_generated: 1,
            actual: 0,
            paused: true,
            status: 0,
            presented_second: 0,
            presented_shown: 0,
            have_second: false,
            second_at: Instant::now(),
            last_color: None,
            error: None,
        }
    }

    pub(crate) fn requested(&self) -> bool {
        self.requested
    }

    pub(crate) fn loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn set_wanted(&mut self, generated: u32) {
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
        self.error = Some(error);
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
                constants: *library
                    .get::<sl::SetConstants>(b"slSetConstants\0")
                    .map_err(|e| e.to_string())?,
            }
        };
        self.api = Some(api);
        self.loaded = true;
        Ok(())
    }

    /// Constants, tags and options for the present that follows this submit.
    /// `token` is Reflex's frame token. A null token cannot mark the frame.
    pub(crate) fn record(
        &mut self,
        dx12: bool,
        token: *mut c_void,
        encoder: &mut wgpu::CommandEncoder,
        input: Inputs<'_>,
    ) -> Result<(), String> {
        if !self.loaded {
            return Ok(());
        }
        let resized = self.last_color.is_some_and(|prev| prev != input.color);
        self.last_color = Some(input.color);
        let generate = if self.wanted == 0 {
            1
        } else {
            self.wanted.min(self.max_generated.max(1))
        };
        let interpolate = input.allow
            && !resized
            && self.wanted > 0
            && input.depth.is_some()
            && input.motion.is_some()
            && input.constants.is_some()
            && !token.is_null();
        self.actual = if interpolate { generate } else { 0 };
        self.paused = self.wanted > 0 && !interpolate;
        if !token.is_null()
            && let Some(constants) = &input.constants
            && let Some(api) = &self.api
        {
            let constants = super_resolution::native_constants(constants);
            let viewport = sl::Viewport::new();
            // Safety: token is the open Reflex frame. The viewport outlives the call.
            let code = unsafe { (api.constants)(&constants, token, &viewport) };
            if code != 0 {
                self.note(format!("Frame Generation constants failed ({code})"));
            }
        }
        if interpolate {
            if let Err(error) = self.tag(dx12, token, encoder, &input) {
                self.actual = 0;
                self.paused = true;
                let _ = self.apply(dx12, false, 1, &input);
                return Err(error);
            }
        } else if !token.is_null() {
            self.tag_cleared(dx12, token, encoder)?;
        }
        self.apply(dx12, interpolate, generate, &input)
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
            self.note(format!("Frame Generation state failed ({code})"));
            return;
        }
        if state.max_generated > 0 {
            self.max_generated = state.max_generated;
        }
        self.presented_second = self.presented_second.saturating_add(state.presented);
        if self.second_at.elapsed() >= Duration::from_secs(1) {
            self.presented_shown = self.presented_second;
            self.presented_second = 0;
            self.have_second = true;
            self.second_at = Instant::now();
        }
        if state.status != self.status {
            self.status = state.status;
            if state.status != 0 {
                self.note(format!(
                    "Frame Generation status {status:#x}",
                    status = state.status
                ));
            }
        }
    }

    pub(crate) fn line(&self) -> String {
        if !self.loaded {
            return String::new();
        }
        if self.wanted == 0 {
            return "Frame Gen: off".into();
        }
        if self.paused || self.status != 0 || self.actual == 0 {
            return "Frame Gen: paused".into();
        }
        let shown = if self.have_second {
            self.presented_shown
        } else {
            self.presented_second
        };
        format!("Frame Gen: {}x · {shown} shown", self.actual + 1)
    }

    fn note(&mut self, error: String) {
        self.error = Some(error);
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
        if code != 0 {
            self.actual = 0;
            self.paused = self.wanted > 0;
            return Err(format!("Frame Generation options failed ({code})"));
        }
        Ok(())
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

fn null_tag(kind: u32) -> sl::Tag {
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
