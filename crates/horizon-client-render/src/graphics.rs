//! Horizon's own graphics settings, beyond vanilla's options: kept in
//! `horizon-graphics.txt` next to `options.txt` so that file stays exactly
//! vanilla's. Every default reproduces vanilla's rendering; only choices the
//! player makes here depart from it.
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
};

mod dlss;
pub use dlss::{
    DlssPreset, DlssReport, dlss_details, dlss_monitor_enabled, dlss_monitor_lines,
    report_dlss_details, set_dlss_monitor_enabled,
};

pub const FILE: &str = "horizon-graphics.txt";
/// Written beside it before HDR output is switched on and removed once the first HDR frame is
/// presented. Found at the next start, it means HDR output took the game down, so HDR starts off.
pub const HDR_MARKER: &str = "horizon-hdr-pending";

/// What the system reports for the display HDR output goes to, in nits in HDR output's
/// scale: where its SDR white sits (the desktop's white) and the peak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reported {
    pub paper_white: u32,
    pub peak: u32,
}

static REPORTED: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Sets what the system reports (the renderer, when it opens its surface).
pub fn report(reported: Option<Reported>) {
    let (paper_white, peak) = reported.map_or((0, 0), |r| (r.paper_white, r.peak));
    REPORTED[0].store(paper_white, Ordering::Relaxed);
    REPORTED[1].store(peak, Ordering::Relaxed);
}
/// What the system reports, if it does.
pub fn reported() -> Option<Reported> {
    let [paper_white, peak] = REPORTED.each_ref().map(|n| n.load(Ordering::Relaxed));
    (peak > 0).then_some(Reported { paper_white, peak })
}

/// Simple-level presets. `Custom` leaves every option as the player set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Preset {
    Performance,
    Balanced,
    Quality,
    Ultra,
    #[default]
    Custom,
}
impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::Performance,
        Preset::Balanced,
        Preset::Quality,
        Preset::Ultra,
        Preset::Custom,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Preset::Performance => "performance",
            Preset::Balanced => "balanced",
            Preset::Quality => "quality",
            Preset::Ultra => "ultra",
            Preset::Custom => "custom",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Preset::Performance => "Performance",
            Preset::Balanced => "Balanced",
            Preset::Quality => "Quality",
            Preset::Ultra => "Ultra",
            Preset::Custom => "Custom",
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or_default()
    }
}

/// How the swapchain presents frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PresentMode {
    /// Vanilla's choice: Fifo with VSync on, else the first of Immediate,
    /// Mailbox and Fifo the surface supports. The other modes apply only
    /// with VSync off: VSync on always presents with Fifo.
    #[default]
    Auto,
    Fifo,
    Mailbox,
    Immediate,
}
impl PresentMode {
    pub const ALL: [PresentMode; 4] = [
        PresentMode::Auto,
        PresentMode::Fifo,
        PresentMode::Mailbox,
        PresentMode::Immediate,
    ];
    pub fn name(self) -> &'static str {
        match self {
            PresentMode::Auto => "auto",
            PresentMode::Fifo => "fifo",
            PresentMode::Mailbox => "mailbox",
            PresentMode::Immediate => "immediate",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PresentMode::Auto => "Auto (VSync)",
            PresentMode::Fifo => "FIFO",
            PresentMode::Mailbox => "Mailbox",
            PresentMode::Immediate => "Immediate",
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.name() == s)
            .unwrap_or_default()
    }
}

/// The graphics API the renderer runs on. Windows picks Vulkan, DirectX 12 or
/// OpenGL; Linux picks Vulkan or OpenGL; macOS always uses its one backend
/// (Metal). Applies at the next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GraphicsApi {
    /// Vulkan, then DirectX 12 (Windows), then OpenGL when neither has a surface, adapter or device.
    #[default]
    Auto,
    Vulkan,
    Dx12,
    /// OpenGL, for GPUs with no Vulkan or DirectX 12 driver (Windows and Linux).
    OpenGl,
}
impl GraphicsApi {
    pub const ALL: [GraphicsApi; 4] = [
        GraphicsApi::Auto,
        GraphicsApi::Vulkan,
        GraphicsApi::Dx12,
        GraphicsApi::OpenGl,
    ];
    /// The APIs this system offers, in the order the setting cycles through
    /// them: DirectX 12 is Windows' third backend, and macOS has no choice.
    pub fn choices() -> &'static [GraphicsApi] {
        const WINDOWS: [GraphicsApi; 4] = [
            GraphicsApi::Auto,
            GraphicsApi::Vulkan,
            GraphicsApi::Dx12,
            GraphicsApi::OpenGl,
        ];
        const LINUX: [GraphicsApi; 3] =
            [GraphicsApi::Auto, GraphicsApi::Vulkan, GraphicsApi::OpenGl];
        const MACOS: [GraphicsApi; 1] = [GraphicsApi::Auto];
        if cfg!(windows) {
            &WINDOWS
        } else if cfg!(target_os = "macos") {
            &MACOS
        } else {
            &LINUX
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            GraphicsApi::Auto => "auto",
            GraphicsApi::Vulkan => "vulkan",
            GraphicsApi::Dx12 => "dx12",
            GraphicsApi::OpenGl => "opengl",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            GraphicsApi::Auto => "Auto",
            GraphicsApi::Vulkan => "Vulkan",
            GraphicsApi::Dx12 => "DirectX 12",
            GraphicsApi::OpenGl => "OpenGL",
        }
    }
    /// The API a name stands for, ignoring case; `None` for anything else.
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|api| api.name().eq_ignore_ascii_case(s.trim()))
    }
    /// `HORIZON_GRAPHICS_API` (auto, vulkan, dx12 or opengl) if set, else the setting.
    pub fn with_env(self) -> Self {
        std::env::var("HORIZON_GRAPHICS_API")
            .ok()
            .and_then(|v| Self::from_name(&v))
            .unwrap_or(self)
    }
}

/// What the frame limiter does in the last half millisecond before a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pacing {
    /// Sleep the whole wait: no CPU spent spinning, a little less precise.
    #[default]
    Efficient,
    /// Vanilla's `FramerateLimiter`: sleep, then spin until the deadline.
    Precise,
}

/// The performance overlay drawn over the HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Overlay {
    #[default]
    Off,
    /// Frames per second and frame time.
    Fps,
    /// The numbers and a graph of recent frame times.
    Graph,
}

/// How finely far terrain is drawn (`FarDetail` in the LOD options).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FarDetail {
    /// Each tile level starts at eight times its size from the camera.
    #[default]
    Full,
    /// Coarser levels start at half that distance: about a quarter of the
    /// far quads, for GPUs that far terrain slows down. Changes the picture.
    Reduced,
}
impl FarDetail {
    pub fn name(self) -> &'static str {
        match self {
            FarDetail::Full => "full",
            FarDetail::Reduced => "reduced",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FarDetail::Full => "Full",
            FarDetail::Reduced => "Reduced",
        }
    }
    fn parse(s: &str) -> Self {
        if s == "reduced" {
            FarDetail::Reduced
        } else {
            FarDetail::Full
        }
    }
}

/// Shared quality for Standard and DLSS. Native is 1×. There is no Off:
/// turning the picture path off is [`Upscaler::Off`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Upscaling {
    #[default]
    Native,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
}
impl Upscaling {
    pub const ALL: [Upscaling; 5] = [
        Upscaling::Native,
        Upscaling::Quality,
        Upscaling::Balanced,
        Upscaling::Performance,
        Upscaling::UltraPerformance,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Upscaling::Native => "native",
            Upscaling::Quality => "quality",
            Upscaling::Balanced => "balanced",
            Upscaling::Performance => "performance",
            Upscaling::UltraPerformance => "ultraPerformance",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Upscaling::Native => "Native",
            Upscaling::Quality => "Quality",
            Upscaling::Balanced => "Balanced",
            Upscaling::Performance => "Performance",
            Upscaling::UltraPerformance => "Ultra Performance",
        }
    }
    /// Output pixels per rendered pixel, along each axis. Native is 1×.
    pub fn ratio(self) -> f32 {
        match self {
            Upscaling::Native => 1.0,
            Upscaling::Quality => 1.5,
            Upscaling::Balanced => 1.7,
            Upscaling::Performance => 2.0,
            Upscaling::UltraPerformance => 3.0,
        }
    }
    /// The built-in upscaler renders smaller and lets TAA rebuild the picture.
    pub fn scales(self) -> bool {
        !matches!(self, Upscaling::Native)
    }
    /// The DLSS mode of the same quality. Native is full-resolution DLAA.
    pub fn as_dlss(self) -> Dlss {
        match self {
            Upscaling::Native => Dlss::Dlaa,
            Upscaling::Quality => Dlss::Quality,
            Upscaling::Balanced => Dlss::Balanced,
            Upscaling::Performance => Dlss::Performance,
            Upscaling::UltraPerformance => Dlss::UltraPerformance,
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|u| u.name() == s)
            .unwrap_or_default()
    }
}

/// Which picture path runs. Off is full resolution with no upscaler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Upscaler {
    /// Full resolution. No built-in upscaler and no DLSS.
    Off,
    /// The built-in upscaler. A quality below Native turns TAA on.
    #[default]
    Standard,
    /// Full-resolution DLSS anti-aliasing.
    Dlaa,
    /// DLSS Super Resolution at [`Upscaling`].
    Dlss,
}
impl Upscaler {
    pub const ALL: [Self; 4] = [Self::Off, Self::Standard, Self::Dlaa, Self::Dlss];
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Standard => "standard",
            Self::Dlaa => "dlaa",
            Self::Dlss => "dlss",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Standard => "Standard",
            Self::Dlaa => "DLAA",
            Self::Dlss => "DLSS",
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == s)
            .unwrap_or_default()
    }
}

/// NVIDIA mode stored for older files. The menu choice is [`Upscaler`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dlss {
    #[default]
    Off,
    Quality,
    Balanced,
    Performance,
    UltraPerformance,
    /// Full-resolution DLSS. This is DLSS plus [`Upscaling::Native`].
    Dlaa,
}
impl Dlss {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Quality,
        Self::Balanced,
        Self::Performance,
        Self::UltraPerformance,
        Self::Dlaa,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Quality => "quality",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
            Self::UltraPerformance => "ultraPerformance",
            Self::Dlaa => "dlaa",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Quality => "Quality",
            Self::Balanced => "Balanced",
            Self::Performance => "Performance",
            Self::UltraPerformance => "Ultra Performance",
            Self::Dlaa => "DLAA",
        }
    }
    pub fn mode(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Quality => 3,
            Self::Balanced => 2,
            Self::Performance => 1,
            Self::UltraPerformance => 4,
            Self::Dlaa => 6,
        }
    }
    /// Quality shared with the built-in upscaler, when DLSS is the upscaler.
    pub fn quality(self) -> Option<Upscaling> {
        Some(match self {
            Self::Off => return None,
            Self::Dlaa => Upscaling::Native,
            Self::Quality => Upscaling::Quality,
            Self::Balanced => Upscaling::Balanced,
            Self::Performance => Upscaling::Performance,
            Self::UltraPerformance => Upscaling::UltraPerformance,
        })
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == s)
            .unwrap_or_default()
    }
}

/// NVIDIA Reflex. Off until the player turns it on. The SDK loads only at the
/// next start, and it never replaces the game's own frame cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Reflex {
    #[default]
    Off,
    On,
    Boost,
}
impl Reflex {
    pub const ALL: [Self; 3] = [Self::Off, Self::On, Self::Boost];
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Boost => "boost",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::On => "On",
            Self::Boost => "On + Boost",
        }
    }
    /// Streamline's ReflexMode: off, low latency, low latency with boost.
    pub fn sdk(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::On => 1,
            Self::Boost => 2,
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == s)
            .unwrap_or_default()
    }
}

/// DLSS Frame Generation. Off until the player turns it on. The first On needs
/// a restart, because the plugin loads with the device. Turning it on turns
/// Reflex on. Turning it off leaves Reflex where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameGen {
    #[default]
    Off,
    /// One generated frame per real frame.
    X2,
    X3,
    X4,
    X6,
}
impl FrameGen {
    pub const ALL: [Self; 5] = [Self::Off, Self::X2, Self::X3, Self::X4, Self::X6];
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::X2 => "2x",
            Self::X3 => "3x",
            Self::X4 => "4x",
            Self::X6 => "6x",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::X2 => "2x",
            Self::X3 => "3x",
            Self::X4 => "4x",
            Self::X6 => "6x",
        }
    }
    /// Extra frames between real ones. 2× is 1. Zero is off.
    pub fn generated(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::X2 => 1,
            Self::X3 => 2,
            Self::X4 => 3,
            Self::X6 => 5,
        }
    }
    /// Generated frames the driver can accept. `max` is that cap, and it is
    /// treated as 1 until the driver reports a higher one.
    pub fn clamp_generated(self, max: u32) -> u32 {
        let wanted = self.generated();
        if wanted == 0 {
            0
        } else {
            wanted.min(max.max(1))
        }
    }
    fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == s)
            .unwrap_or_default()
    }
}
static DLSS_STATUS: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
pub fn report_dlss(status: impl Into<String>) {
    *DLSS_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = status.into();
}
pub fn dlss_status() -> String {
    let s = DLSS_STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if s.is_empty() {
        "Off".into()
    } else {
        s.clone()
    }
}

static REFLEX_LINE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// The latency line for the performance overlay. Empty hides it.
pub fn report_reflex(line: impl Into<String>) {
    *REFLEX_LINE.lock().unwrap_or_else(|e| e.into_inner()) = line.into();
}
pub fn reflex_overlay_line() -> Option<String> {
    let line = REFLEX_LINE.lock().unwrap_or_else(|e| e.into_inner());
    if line.is_empty() {
        None
    } else {
        Some(line.clone())
    }
}

static FRAMEGEN_LINE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// The frame-generation line for the performance overlay. Empty hides it.
pub fn report_framegen(line: impl Into<String>) {
    *FRAMEGEN_LINE.lock().unwrap_or_else(|e| e.into_inner()) = line.into();
}
pub fn framegen_overlay_line() -> Option<String> {
    let line = FRAMEGEN_LINE.lock().unwrap_or_else(|e| e.into_inner());
    if line.is_empty() {
        None
    } else {
        Some(line.clone())
    }
}

static REDUCED_SUGGESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Whether the settings suggest Reduced far detail: set when the renderer
/// starts, on the GPUs `lod_capped` names (far terrain slows them most).
pub fn suggest_reduced_far_detail(on: bool) {
    REDUCED_SUGGESTED.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn reduced_far_detail_suggested() -> bool {
    REDUCED_SUGGESTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The name the options screen waits on to bind `Graphics::hdr_key`.
pub const HDR_KEY: &str = "horizon.hdrSettings";
static HDR_OFFERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Whether the display path offers HDR output (HDR10 or scRGB): set when the renderer
/// opens its surface; the settings open HDR's screen only then.
pub fn offer_hdr(on: bool) {
    HDR_OFFERED.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn hdr_offered() -> bool {
    HDR_OFFERED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The farthest LOD Distance, in chunks, on integrated GPUs other than
/// Apple's: they share the CPU's memory bandwidth, and past it far terrain
/// costs them more frame time than it is worth.
pub const INTEGRATED_LOD_DISTANCE: u32 = 128;
/// Whether `INTEGRATED_LOD_DISTANCE` applies to this adapter. Apple's GPUs
/// are integrated too, with memory and bandwidth enough for every distance.
pub fn lod_capped(info: &wgpu::AdapterInfo) -> bool {
    info.device_type == wgpu::DeviceType::IntegratedGpu && !info.name.starts_with("Apple")
}
/// A LOD distance as far terrain uses it on this adapter.
pub fn lod_distance_capped(distance: u32, info: &wgpu::AdapterInfo) -> u32 {
    let distance = distance.min(1024);
    if lod_capped(info) {
        distance.min(INTEGRATED_LOD_DISTANCE)
    } else {
        distance
    }
}
impl Graphics {
    /// `lod_distance` as far terrain uses it on this adapter.
    pub fn lod_distance_on(&self, info: &wgpu::AdapterInfo) -> u32 {
        lod_distance_capped(self.lod_distance, info)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Graphics {
    pub preset: Preset,
    /// Distant surface terrain range, in chunks; 0 disables LODs.
    pub lod_distance: u32,
    /// How finely far terrain is drawn; Full unless the player picks Reduced.
    pub far_detail: FarDetail,
    /// Skip sections hidden behind closed ones (vanilla's occlusion graph).
    pub cave_culling: bool,
    /// Skip drawing entities hidden behind opaque terrain.
    pub entity_culling: bool,
    /// Meshed sections uploaded per frame; 0 uploads all that are ready.
    pub chunk_updates: u32,
    /// Section mesh bytes uploaded per frame, in KiB; 0 is unlimited.
    pub upload_budget_kib: u32,
    pub sky: bool,
    pub fog: bool,
    pub weather: bool,
    pub animated_textures: bool,
    pub present_mode: PresentMode,
    pub pacing: Pacing,
    /// Chunk worker threads; 0 uses every core but one. Applies at the next start.
    pub workers: u32,
    pub overlay: Overlay,
    /// World resolution as a percentage of the window's (50..=200).
    pub resolution_scale: u32,
    /// FXAA over the world.
    pub fxaa: bool,
    /// TAA over the world (FXAA is off while it is on).
    pub taa: bool,
    /// Quality shared by Standard and DLSS. Native is 1×. Off is not a quality.
    pub upscaling: Upscaling,
    /// Off, Standard, DLAA or DLSS. Off renders at full resolution.
    pub upscaler: Upscaler,
    /// Effective NVIDIA mode. Off unless the upscaler is DLAA or DLSS.
    pub dlss: Dlss,
    pub dlss_preset: DlssPreset,
    pub dlss_monitor: bool,
    /// Reflex low latency. Off until chosen. The first On needs a restart.
    pub reflex: Reflex,
    /// Frame generation. Off until chosen. The first On needs a restart and
    /// turns Reflex on.
    pub framegen: FrameGen,
    /// HDR output (HDR10, or scRGB), where the display path offers it.
    pub hdr: bool,
    /// HDR paper white (where SDR white sits), in nits; 0 is Auto: the desktop's SDR
    /// white, as the system reports it ([`Graphics::paper_white_nits`]).
    pub hdr_paper_white: u32,
    /// HDR peak brightness the world's highlights reach, in nits; 0 is Auto:
    /// what the system reports for the display ([`Graphics::peak_nits`]).
    pub hdr_peak: u32,
    /// HDR black level: the display's darkest black, in hundredths of a nit (0 for
    /// an OLED's true black). Shadows are lifted onto it, and the HDR metadata says it.
    pub hdr_black_level: u32,
    /// HDR brightness of the HUD and menus, in percent of paper white (25..=100); the
    /// first-person hand stays at the world's.
    pub hdr_ui_brightness: u32,
    /// A vanilla key name (`HDR_KEY`): opens HDR's settings from the game. Unbound by default.
    pub hdr_key: String,
    /// Battery mode: at most 60 frames per second in a world and 30 in menus
    /// or behind other windows, and half the worker threads. Off by default;
    /// it changes when frames are drawn, never what they show or the ticks.
    pub battery: bool,
    /// The graphics API (Windows only; see `GraphicsApi`).
    pub api: GraphicsApi,
    /// The shader pack picked in `shaderpacks/` (`shader_pack_list`); empty: none. Its key is
    /// not `shaderPack`, which versions before the clean-room rewrite saved and which stays
    /// ignored.
    pub shader_pack: String,
}
impl Default for Graphics {
    fn default() -> Self {
        Self {
            preset: Preset::Custom,
            lod_distance: 0,
            far_detail: FarDetail::Full,
            cave_culling: true,
            entity_culling: false,
            chunk_updates: 0,
            upload_budget_kib: 0,
            sky: true,
            fog: true,
            weather: true,
            animated_textures: true,
            present_mode: PresentMode::Auto,
            pacing: Pacing::Efficient,
            workers: 0,
            overlay: Overlay::Off,
            resolution_scale: 100,
            fxaa: false,
            taa: false,
            upscaling: Upscaling::Native,
            upscaler: Upscaler::Standard,
            dlss: Dlss::Off,
            dlss_preset: DlssPreset::Auto,
            dlss_monitor: false,
            reflex: Reflex::Off,
            framegen: FrameGen::Off,
            hdr: false,
            hdr_paper_white: 0,
            hdr_peak: 0,
            hdr_black_level: 0,
            hdr_ui_brightness: 100,
            hdr_key: "key.keyboard.unknown".into(),
            battery: false,
            api: GraphicsApi::Auto,
            shader_pack: String::new(),
        }
    }
}
fn bool_text(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}
impl Graphics {
    pub fn load(dir: &Path) -> Self {
        let mut g: Self = fs::read_to_string(dir.join(FILE))
            .map(|t| Self::parse(&t))
            .unwrap_or_default();
        if fs::remove_file(dir.join(HDR_MARKER)).is_ok() && g.hdr {
            g.hdr = false;
            g.save(dir);
            log::warn!(
                "the last start ended before HDR output showed a frame; HDR is off now (graphics option hdr, in {FILE})"
            );
        }
        g
    }
    pub fn parse(text: &str) -> Self {
        let mut g = Self::default();
        // `upscaling:off` is the old quality row. It means the upscaler is Off,
        // and it must not be read as Native. An explicit `upscaler` wins.
        let mut saw_upscaler = false;
        let mut legacy_off = false;
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let flag = |d: bool| match value {
                "true" => true,
                "false" => false,
                _ => d,
            };
            let number = |d: u32| value.parse().unwrap_or(d);
            match key.trim() {
                "preset" => g.preset = Preset::parse(value),
                "lodDistance" => g.lod_distance = number(g.lod_distance).min(1024),
                "farDetail" => g.far_detail = FarDetail::parse(value),
                "caveCulling" => g.cave_culling = flag(g.cave_culling),
                "entityCulling" => g.entity_culling = flag(g.entity_culling),
                "chunkUpdates" => g.chunk_updates = number(g.chunk_updates).min(512),
                "uploadBudgetKiB" => g.upload_budget_kib = number(g.upload_budget_kib).min(65536),
                "sky" => g.sky = flag(g.sky),
                "fog" => g.fog = flag(g.fog),
                "weather" => g.weather = flag(g.weather),
                "animatedTextures" => g.animated_textures = flag(g.animated_textures),
                "presentMode" => g.present_mode = PresentMode::parse(value),
                "framePacing" => {
                    g.pacing = if value == "precise" {
                        Pacing::Precise
                    } else {
                        Pacing::Efficient
                    }
                }
                "workers" => g.workers = number(g.workers).min(64),
                "resolutionScale" => g.resolution_scale = number(g.resolution_scale).clamp(50, 200),
                "fxaa" => g.fxaa = flag(g.fxaa),
                "taa" => g.taa = flag(g.taa),
                "upscaling" => {
                    legacy_off = value == "off";
                    if !legacy_off {
                        g.upscaling = Upscaling::parse(value);
                    }
                }
                "upscaler" => {
                    g.upscaler = Upscaler::parse(value);
                    saw_upscaler = true;
                }
                "dlss" => g.dlss = Dlss::parse(value),
                "dlssPreset" => g.dlss_preset = DlssPreset::parse(value),
                "dlssMonitor" => g.dlss_monitor = flag(g.dlss_monitor),
                "reflex" => g.reflex = Reflex::parse(value),
                "framegen" => g.framegen = FrameGen::parse(value),
                "hdr" => g.hdr = flag(g.hdr),
                "hdrPaperWhite" => {
                    g.hdr_paper_white = match number(g.hdr_paper_white) {
                        0 => 0,
                        n => n.clamp(80, 480),
                    }
                }
                "hdrPeak" => {
                    g.hdr_peak = match number(g.hdr_peak) {
                        0 => 0,
                        n => n.clamp(200, 2000),
                    }
                }
                "hdrBlackLevel" => g.hdr_black_level = number(g.hdr_black_level).min(100),
                "hdrUiBrightness" => {
                    g.hdr_ui_brightness = number(g.hdr_ui_brightness).clamp(25, 100)
                }
                "hdrKey" if value.starts_with("key.") => g.hdr_key = value.to_owned(),
                "batteryMode" => g.battery = flag(g.battery),
                "graphicsApi" => g.api = GraphicsApi::from_name(value).unwrap_or_default(),
                "pickedShaderPack" => g.shader_pack = value.to_owned(),
                "overlay" => {
                    g.overlay = match value {
                        "fps" => Overlay::Fps,
                        "graph" => Overlay::Graph,
                        _ => Overlay::Off,
                    }
                }
                _ => {}
            }
        }
        // Older menus stored the DLSS quality separately. That value was the
        // one on screen, unless the quality row itself said Off.
        if !saw_upscaler {
            if legacy_off {
                g.upscaler = Upscaler::Off;
                if let Some(quality) = g.dlss.quality() {
                    g.upscaling = quality;
                }
            } else if let Some(quality) = g.dlss.quality() {
                g.upscaling = quality;
                g.fxaa = false;
                g.taa = false;
                g.upscaler = if g.dlss == Dlss::Dlaa {
                    Upscaler::Dlaa
                } else {
                    Upscaler::Dlss
                };
            } else {
                g.upscaler = Upscaler::Standard;
            }
        }
        g.dlss = g.dlss_mode();
        // A hand-edited file can ask for frames without Reflex. The driver
        // rejects that, so the saved mode and the SDK both start with Reflex on.
        if g.framegen != FrameGen::Off && g.reflex == Reflex::Off {
            g.reflex = Reflex::On;
        }
        g
    }
    /// DLAA or DLSS is selected, so FXAA and TAA stay off.
    pub fn dlss_selected(&self) -> bool {
        matches!(self.upscaler, Upscaler::Dlaa | Upscaler::Dlss)
    }
    /// Mode sent to DLSS. Off and Standard send nothing. DLAA is always 1×.
    /// DLSS uses [`Upscaling`], and Native there is full-resolution DLAA.
    pub fn dlss_mode(&self) -> Dlss {
        match self.upscaler {
            Upscaler::Off | Upscaler::Standard => Dlss::Off,
            Upscaler::Dlaa => Dlss::Dlaa,
            Upscaler::Dlss => self.upscaling.as_dlss(),
        }
    }
    /// Rendered-size ratio. Off and DLAA stay at 1×. Standard uses the quality
    /// only while TAA is rebuilding the picture. DLSS always uses the quality.
    pub fn render_scale(&self) -> f32 {
        match self.upscaler {
            Upscaler::Dlss => self.upscaling.ratio(),
            Upscaler::Standard if self.taa && self.upscaling.scales() => self.upscaling.ratio(),
            _ => 1.0,
        }
    }
    /// Pick the upscaler. DLAA and DLSS clear anti-aliasing. Standard turns TAA
    /// on when the quality renders a smaller picture. Off drops that TAA.
    pub fn set_upscaler(&mut self, upscaler: Upscaler) {
        let previous = self.upscaler;
        self.upscaler = upscaler;
        if upscaler == Upscaler::Off && previous == Upscaler::Standard && self.upscaling.scales() {
            self.taa = false;
        }
        if matches!(upscaler, Upscaler::Dlaa | Upscaler::Dlss) {
            self.fxaa = false;
            self.taa = false;
        }
        if upscaler == Upscaler::Standard && self.upscaling.scales() {
            self.fxaa = false;
            self.taa = true;
        }
        self.dlss = self.dlss_mode();
    }
    /// One quality for Standard and DLSS. DLAA stays 1× and remembers it.
    /// Off remembers it too, and still renders at full resolution.
    pub fn set_upscale_quality(&mut self, quality: Upscaling) {
        self.upscaling = quality;
        if self.upscaler == Upscaler::Standard && quality.scales() {
            self.fxaa = false;
            self.taa = true;
        }
        if self.dlss_selected() {
            self.fxaa = false;
            self.taa = false;
        }
        self.dlss = self.dlss_mode();
    }
    pub fn serialize(&self) -> String {
        format!(
            "preset:{}\nlodDistance:{}\nfarDetail:{}\ncaveCulling:{}\nentityCulling:{}\nchunkUpdates:{}\nuploadBudgetKiB:{}\nsky:{}\nfog:{}\nweather:{}\nanimatedTextures:{}\npresentMode:{}\nframePacing:{}\nworkers:{}\noverlay:{}\nresolutionScale:{}\nfxaa:{}\ntaa:{}\nupscaling:{}\nupscaler:{}\ndlss:{}\ndlssPreset:{}\ndlssMonitor:{}\nreflex:{}\nframegen:{}\nhdr:{}\nhdrPaperWhite:{}\nhdrPeak:{}\nhdrBlackLevel:{}\nhdrUiBrightness:{}\nhdrKey:{}\nbatteryMode:{}\ngraphicsApi:{}\npickedShaderPack:{}\n",
            self.preset.name(),
            self.lod_distance,
            self.far_detail.name(),
            bool_text(self.cave_culling),
            bool_text(self.entity_culling),
            self.chunk_updates,
            self.upload_budget_kib,
            bool_text(self.sky),
            bool_text(self.fog),
            bool_text(self.weather),
            bool_text(self.animated_textures),
            self.present_mode.name(),
            match self.pacing {
                Pacing::Efficient => "efficient",
                Pacing::Precise => "precise",
            },
            self.workers,
            match self.overlay {
                Overlay::Off => "off",
                Overlay::Fps => "fps",
                Overlay::Graph => "graph",
            },
            self.resolution_scale,
            bool_text(self.fxaa),
            bool_text(self.taa),
            self.upscaling.name(),
            self.upscaler.name(),
            self.dlss.name(),
            self.dlss_preset.name(),
            bool_text(self.dlss_monitor),
            self.reflex.name(),
            self.framegen.name(),
            bool_text(self.hdr),
            self.hdr_paper_white,
            self.hdr_peak,
            self.hdr_black_level,
            self.hdr_ui_brightness,
            self.hdr_key,
            bool_text(self.battery),
            self.api.name(),
            self.shader_pack,
        )
    }
    /// Binds Horizon's graphics key `name` (`HDR_KEY`); false for another name.
    pub fn set_key(&mut self, name: &str, value: String) -> bool {
        match name {
            HDR_KEY => self.hdr_key = value,
            _ => return false,
        }
        true
    }
    pub fn save(&self, dir: &Path) {
        let _ = fs::write(dir.join(FILE), self.serialize());
    }
    /// Paper white, in nits: the player's, or on Auto the desktop's SDR white as the
    /// system reports it, else 203 (BT.2408's reference white).
    pub fn paper_white_nits(&self) -> u32 {
        match self.hdr_paper_white {
            0 => reported().map_or(203, |r| r.paper_white),
            n => n,
        }
    }
    /// The peak the world's highlights reach, in nits: the player's, or on Auto
    /// what the system reports, else 1000.
    pub fn peak_nits(&self) -> u32 {
        match self.hdr_peak {
            0 => reported().map_or(1000, |r| r.peak),
            n => n,
        }
    }
    /// Worker threads for chunk generation and meshing.
    pub fn worker_threads(&self) -> usize {
        if self.workers > 0 {
            return self.workers as usize;
        }
        // Vanilla's count (Util.maxAllowedExecutorThreads: every core but
        // one). The pool never takes every core: a pool that fills them all
        // starves the render and server threads, and the frame rate collapses
        // while the workers saturate the CPU and its memory bandwidth. One
        // core left is enough with the workers behind those two threads
        // (horizon_threads: niced on Linux; on macOS and Windows the server
        // and render threads are raised above four cores, where vanilla
        // raises its server thread). Leaving two idled a core of a 4-thread
        // laptop while a world loaded: a teleport there took ~15% longer.
        let auto = std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .saturating_sub(1)
            .max(1);
        if self.battery { auto.div_ceil(2) } else { auto }
    }
    /// The frame cap battery mode adds: 60 in a world, 30 in menus without a
    /// world or while another window has focus.
    pub fn battery_limit(&self, limit: i32, in_world: bool, focused: bool) -> i32 {
        if !self.battery {
            limit
        } else if in_world && focused {
            limit.min(60)
        } else {
            limit.min(30)
        }
    }
}

/// A preset's values for vanilla's options and Horizon's own.
pub fn apply_preset(preset: Preset, s: &mut crate::settings::Settings) {
    let (rd, sim, clouds, particles, shadows, blend, mipmaps, entity, leaves, weather_r) =
        match preset {
            Preset::Performance => (8, 6, "false", 2, false, 0, 0, 0.5, false, 3),
            Preset::Balanced => (12, 8, "fast", 1, true, 1, 2, 0.75, true, 5),
            Preset::Quality => (16, 12, "true", 0, true, 2, 4, 1.0, true, 10),
            Preset::Ultra => (24, 12, "true", 0, true, 3, 4, 1.5, true, 10),
            Preset::Custom => return,
        };
    s.render_distance = rd;
    s.simulation_distance = sim;
    s.clouds = clouds.into();
    s.particles = particles;
    s.entity_shadows = shadows;
    s.biome_blend = blend;
    s.mipmap_levels = mipmaps;
    s.entity_distance = entity;
    s.cutout_leaves = leaves;
    s.weather_radius = weather_r;
    s.graphics_preset = "custom".into();
    let g = &mut s.horizon;
    g.preset = preset;
    g.cave_culling = true;
    g.animated_textures = true;
    g.chunk_updates = 0;
    g.upload_budget_kib = 0;
    g.sky = true;
    g.fog = true;
    g.weather = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_choice_matches_the_system() {
        let choices = GraphicsApi::choices();
        if cfg!(windows) {
            assert_eq!(choices, GraphicsApi::ALL);
        } else if cfg!(target_os = "macos") {
            assert_eq!(choices, [GraphicsApi::Auto]);
        } else {
            assert_eq!(
                choices,
                [GraphicsApi::Auto, GraphicsApi::Vulkan, GraphicsApi::OpenGl]
            );
        }
    }

    #[test]
    fn round_trips() {
        let g = Graphics {
            preset: Preset::Balanced,
            lod_distance: 256,
            far_detail: FarDetail::Reduced,
            cave_culling: false,
            entity_culling: true,
            chunk_updates: 12,
            upload_budget_kib: 4096,
            sky: false,
            fog: false,
            weather: false,
            animated_textures: false,
            present_mode: PresentMode::Mailbox,
            pacing: Pacing::Precise,
            workers: 6,
            overlay: Overlay::Graph,
            resolution_scale: 75,
            fxaa: false,
            taa: false,
            upscaling: Upscaling::Quality,
            upscaler: Upscaler::Dlss,
            dlss: Dlss::Quality,
            dlss_preset: DlssPreset::M,
            dlss_monitor: true,
            reflex: Reflex::Boost,
            framegen: FrameGen::X4,
            hdr: true,
            hdr_paper_white: 240,
            hdr_peak: 1400,
            hdr_black_level: 12,
            hdr_ui_brightness: 60,
            hdr_key: "key.keyboard.h".into(),
            battery: true,
            api: GraphicsApi::Dx12,
            shader_pack: "Golden Hour".into(),
        };
        assert_eq!(Graphics::parse(&g.serialize()), g);
        assert_eq!(Graphics::parse(""), Graphics::default());
        // A file from the old split controls keeps the DLSS quality that was shown.
        let old = Graphics::parse("dlss:performance\nupscaling:native\ntaa:true\nfxaa:true\n");
        assert_eq!(old.upscaler, Upscaler::Dlss);
        assert_eq!(old.dlss, Dlss::Performance);
        assert_eq!(old.upscaling, Upscaling::Performance);
        assert!(!old.taa);
        assert!(!old.fxaa);
        // The old quality Off is the upscaler Off. Quality is remembered, and
        // an unknown quality name stays Native.
        let off = Graphics::parse("upscaling:off\ndlss:quality\ntaa:true\n");
        assert_eq!(off.upscaler, Upscaler::Off);
        assert_eq!(off.upscaling, Upscaling::Quality);
        assert_eq!(off.dlss, Dlss::Off);
        assert_eq!(off.dlss_mode(), Dlss::Off);
        assert_eq!(off.render_scale(), 1.0);
        assert!(off.taa);
        let dlaa = Graphics::parse("upscaler:dlaa\nupscaling:quality\n");
        assert_eq!(dlaa.upscaler, Upscaler::Dlaa);
        assert_eq!(dlaa.upscaling, Upscaling::Quality);
        assert_eq!(dlaa.dlss_mode(), Dlss::Dlaa);
        assert_eq!(dlaa.render_scale(), 1.0);
        assert_eq!(
            Graphics::parse("upscaling:nope\n").upscaling,
            Upscaling::Native
        );
        assert_eq!(
            Graphics::parse("upscaler:nope\n").upscaler,
            Upscaler::Standard
        );
        assert_eq!(Graphics::parse("reflex:boost\n").reflex, Reflex::Boost);
        assert_eq!(Graphics::parse("reflex:on\n").reflex, Reflex::On);
        assert_eq!(Graphics::parse("reflex:nope\n").reflex, Reflex::Off);
        assert_eq!(Graphics::parse("framegen:4x\n").framegen, FrameGen::X4);
        assert_eq!(Graphics::parse("framegen:4x\n").reflex, Reflex::On);
        assert_eq!(
            Graphics::parse("framegen:6x\nreflex:boost\n").reflex,
            Reflex::Boost
        );
        assert_eq!(Graphics::parse("framegen:nope\n").framegen, FrameGen::Off);
        assert_eq!(FrameGen::X6.clamp_generated(1), 1);
        assert_eq!(FrameGen::X6.clamp_generated(5), 5);
        assert_eq!(FrameGen::X4.clamp_generated(0), 1);
        assert_eq!(FrameGen::Off.clamp_generated(5), 0);
        let mut chosen = Graphics::default();
        chosen.set_upscaler(Upscaler::Off);
        chosen.set_upscale_quality(Upscaling::Quality);
        assert_eq!(chosen.upscaler, Upscaler::Off);
        assert_eq!(chosen.dlss_mode(), Dlss::Off);
        assert_eq!(chosen.render_scale(), 1.0);
        chosen.set_upscaler(Upscaler::Dlss);
        assert_eq!(chosen.dlss_mode(), Dlss::Quality);
        assert_eq!(chosen.render_scale(), 1.5);
        chosen.set_upscaler(Upscaler::Dlaa);
        assert_eq!(chosen.dlss_mode(), Dlss::Dlaa);
        assert_eq!(chosen.render_scale(), 1.0);
        assert_eq!(chosen.upscaling, Upscaling::Quality);
        // 0 is Auto; anything else is a peak the slider reaches.
        assert_eq!(Graphics::parse("hdrPeak:0\n").hdr_peak, 0);
        assert_eq!(Graphics::parse("hdrPeak:50\n").hdr_peak, 200);
        assert_eq!(Graphics::parse("hdrPaperWhite:0\n").hdr_paper_white, 0);
        assert_eq!(Graphics::parse("hdrPaperWhite:5\n").hdr_paper_white, 80);
        // The old menus' Simple / Advanced choice is ignored, and so is the
        // shader pack older versions saved.
        assert_eq!(Graphics::parse("level:advanced\n"), Graphics::default());
        assert_eq!(
            Graphics::parse("shaderPack:Pack.zip\n"),
            Graphics::default()
        );
        assert_eq!(
            Graphics::parse("workers:abc\nsky:maybe\ncullLeaves:true\nentityCulling:maybe\n"),
            Graphics::default()
        );
    }

    #[test]
    fn integrated_gpus_other_than_apple_cap_the_lod_distance() {
        let adapter = |name: &str, device_type| wgpu::AdapterInfo {
            name: name.into(),
            vendor: 0,
            device: 0,
            device_type,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: None,
            limit_bucket: None,
        };
        let hd620 = adapter(
            "Intel(R) HD Graphics 620 (KBL GT2)",
            wgpu::DeviceType::IntegratedGpu,
        );
        let m4 = adapter("Apple M4 Pro", wgpu::DeviceType::IntegratedGpu);
        let discrete = adapter("NVIDIA GeForce RTX 4070", wgpu::DeviceType::DiscreteGpu);
        for (distance, capped) in [(0, 0), (64, 64), (128, 128), (256, 128), (1024, 128)] {
            let g = Graphics {
                lod_distance: distance,
                ..Graphics::default()
            };
            assert_eq!(g.lod_distance_on(&hd620), capped);
            assert_eq!(g.lod_distance_on(&m4), distance);
            assert_eq!(g.lod_distance_on(&discrete), distance);
        }
        assert!(
            crate::graphics_ui::rows(crate::graphics_ui::Level::Simple)
                .iter()
                .any(|row| matches!(row, crate::graphics_ui::Row::Setting(s)
                    if s.text.contains(&format!("{INTEGRATED_LOD_DISTANCE} chunks")))),
            "the LOD Distance description names the cap"
        );
    }

    #[test]
    fn lod_distance_is_opt_in_and_bounded() {
        assert_eq!(Graphics::default().lod_distance, 0);
        for distance in [0, 64, 128, 256, 512, 1024] {
            let g = Graphics::parse(&format!("lodDistance:{distance}\n"));
            assert_eq!(g.lod_distance, distance);
            assert_eq!(Graphics::parse(&g.serialize()), g);
        }
        assert_eq!(Graphics::parse("lodDistance:10000\n").lod_distance, 1024);
        assert_eq!(Graphics::parse("lodDistance:-1\n").lod_distance, 0);
        assert_eq!(Graphics::parse("lodDistance:invalid\n").lod_distance, 0);
    }

    #[test]
    fn far_detail_is_full_unless_reduced_is_picked() {
        assert_eq!(Graphics::default().far_detail, FarDetail::Full);
        assert_eq!(
            Graphics::parse("farDetail:reduced\n").far_detail,
            FarDetail::Reduced
        );
        for text in ["farDetail:full\n", "farDetail:other\n", ""] {
            assert_eq!(Graphics::parse(text).far_detail, FarDetail::Full);
        }
        // No preset changes it.
        for preset in Preset::ALL {
            let mut s = crate::settings::Settings::default();
            apply_preset(preset, &mut s);
            assert_eq!(s.horizon.far_detail, FarDetail::Full, "{preset:?}");
        }
    }

    #[test]
    fn every_preset_keeps_the_animated_textures_on() {
        // The Performance preset used to turn them off, which froze the fire,
        // the water, the lava and the portals (#2064).
        assert!(Graphics::default().animated_textures);
        for preset in Preset::ALL {
            let mut s = crate::settings::Settings::default();
            apply_preset(preset, &mut s);
            assert!(
                s.horizon.animated_textures,
                "{preset:?} turned the animated textures off"
            );
        }
    }

    #[test]
    fn automatic_workers_leave_a_core_as_vanilla_does() {
        let g = Graphics::default();
        assert_eq!(g.workers, 0);
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        assert_eq!(g.worker_threads(), cores.saturating_sub(1).max(1));
        // A manual count is the player's own choice, kept as asked.
        let manual = Graphics { workers: 3, ..g };
        assert_eq!(manual.worker_threads(), 3);
    }

    #[test]
    fn battery_mode_only_lowers_the_frame_cap() {
        let mut g = Graphics::default();
        assert_eq!(g.battery_limit(260, true, true), 260);
        g.battery = true;
        assert_eq!(g.battery_limit(260, true, true), 60);
        assert_eq!(g.battery_limit(30, true, true), 30);
        assert_eq!(g.battery_limit(260, false, true), 30);
        assert_eq!(g.battery_limit(260, true, false), 30);
        assert_eq!(g.battery_limit(10, true, false), 10);
    }
}
