use crate::abi;
use std::{
    ffi::{CStr, CString},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

// Streamline is process-global. Initialization/shutdown must never overlap,
// including two renderer startups or resource reloads.
static SESSION: AtomicBool = AtomicBool::new(false);
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        SESSION.store(false, Ordering::Release);
    }
}

/// Which graphics API this process-global session was initialized for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Vulkan,
    Dx12,
}

#[derive(Debug)]
pub struct Requirements {
    pub instance_extensions: Vec<String>,
    pub device_extensions: Vec<String>,
    pub features12: Vec<String>,
    pub features13: Vec<String>,
    pub compute_queues: u32,
    pub graphics_queues: u32,
}

/// One rendered frame/viewport. SR and FG can request the same constants, but
/// only the first consumer sends them to the SDK. Failure lasts until begin.
#[derive(Default)]
struct RenderFrame {
    active: bool,
    token: usize,
    constants: Option<crate::FrameConstants>,
    failed: bool,
}
impl RenderFrame {
    fn begin(&mut self, token: usize) {
        *self = Self {
            active: true,
            token,
            ..Default::default()
        };
    }

    fn constants(
        &mut self,
        frame: crate::FrameConstants,
        allocate: impl FnOnce() -> Result<usize, String>,
        send: impl FnOnce(usize, &crate::FrameConstants) -> Result<(), String>,
    ) -> Result<usize, String> {
        if !self.active || self.failed {
            return Err("Streamline rendered frame is unavailable".into());
        }
        let result = (|| {
            if !frame.finite() {
                return Err("Streamline common constants are non-finite".into());
            }
            if let Some(previous) = self.constants {
                if previous != frame {
                    return Err("Streamline common constants changed within a frame".into());
                }
                return Ok(self.token);
            }
            if self.token == 0 {
                self.token = allocate()?;
            }
            if self.token == 0 {
                return Err("Streamline returned no frame token".into());
            }
            send(self.token, &frame)?;
            self.constants = Some(frame);
            Ok(self.token)
        })();
        self.failed |= result.is_err();
        result
    }
}

/// Initialized SDK, owned by the render thread. Must remain loaded while its
/// device or a DirectX 12 swapchain proxy is in use, and be shut down after
/// draining GPU work and releasing that proxy, before device destruction.
pub struct Runtime {
    api: Api,
    shutdown: abi::Shutdown,
    requirements: abi::GetRequirements,
    supported: abi::IsSupported,
    set_d3d_device: abi::SetD3DDevice,
    upgrade_interface: abi::UpgradeInterface,
    device: Option<wgpu::Device>,
    instance: Option<wgpu::Instance>,
    /// A failed presentation hook disables consumers without unloading SDK objects.
    presentation: Presentation,
    frame: RenderFrame,
    constants_failures: u8,
    directory: std::path::PathBuf,
    reflex: crate::reflex::Session,
    framegen: crate::frame_gen::Session,
    dlss_requested: bool,
    pub(crate) _library: libloading::os::windows::Library,
    _session: Lease,
}

impl Runtime {
    /// Initialize from an explicitly selected SDK directory. Only the official
    /// production SDK belongs here. No PATH/current-directory DLL lookup.
    /// Vulkan keeps the interposer in charge of its hooks.
    pub fn load(directory: &Path, project_id: &str) -> Result<Self, String> {
        Self::load_for(directory, project_id, Api::Vulkan)
    }

    /// `api` selects Vulkan or DirectX 12 for this process. DirectX 12 uses
    /// manual hooking: the host registers the device and the swapchain.
    /// Super Resolution only. Reflex stays unloaded.
    pub fn load_for(directory: &Path, project_id: &str, api: Api) -> Result<Self, String> {
        Self::load_with(directory, project_id, api, true, false, false)
    }

    /// `dlss`, `reflex` and `framegen` choose which plugins this process loads.
    /// A plugin that is not requested is not loaded. Frame generation also
    /// requests Reflex: the driver rejects it otherwise. DirectX 12 uses
    /// manual hooking.
    pub fn load_with(
        directory: &Path,
        project_id: &str,
        api: Api,
        dlss: bool,
        reflex: bool,
        framegen: bool,
    ) -> Result<Self, String> {
        if !directory.is_absolute() {
            return Err("Streamline requires an absolute SDK directory".into());
        }
        if !valid_project_id(project_id) {
            return Err("Streamline requires a project UUID".into());
        }
        let directory = directory.canonicalize().map_err(|e| e.to_string())?;
        // NVIDIA's signature verifier rejects extended-length paths even when
        // WinVerifyTrust accepts them. Use ordinary absolute paths for the SDK.
        let text = directory.to_string_lossy();
        let directory = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            std::path::PathBuf::from(format!(r"\\{unc}"))
        } else if let Some(local) = text.strip_prefix(r"\\?\") {
            std::path::PathBuf::from(local)
        } else {
            directory
        };
        let project_id = CString::new(project_id).map_err(|e| e.to_string())?;
        SESSION
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "Streamline is already in use")?;
        let guard = Lease;
        let path = directory.join("sl.interposer.dll");
        // Safety: explicit library path, restricted dependency search, only the
        // pinned public ABI is called; library remains alive until shutdown.
        let library = unsafe {
            libloading::os::windows::Library::load_with_flags(&path, 0x00000100 | 0x00000800)
        }
        .map_err(|e| e.to_string())?;
        // Resolve every required symbol before initialization, so a partial
        // incompatible DLL never leaves an initialized process-global SDK.
        let (init, shutdown, requirements, loaded, supported, set_d3d_device, upgrade_interface) = unsafe {
            (
                *library
                    .get::<abi::Init>(b"slInit\0")
                    .map_err(|e| e.to_string())?,
                *library
                    .get::<abi::Shutdown>(b"slShutdown\0")
                    .map_err(|e| e.to_string())?,
                *library
                    .get::<abi::GetRequirements>(b"slGetFeatureRequirements\0")
                    .map_err(|e| e.to_string())?,
                *library
                    .get::<abi::IsLoaded>(b"slIsFeatureLoaded\0")
                    .map_err(|e| e.to_string())?,
                *library
                    .get::<abi::IsSupported>(b"slIsFeatureSupported\0")
                    .map_err(|e| e.to_string())?,
                *library
                    .get::<abi::SetD3DDevice>(b"slSetD3DDevice\0")
                    .map_err(|e| e.to_string())?,
                *library
                    .get::<abi::UpgradeInterface>(b"slUpgradeInterface\0")
                    .map_err(|e| e.to_string())?,
            )
        };
        // sl::RenderAPI: eD3D11 = 0, eD3D12 = 1, eVulkan = 2.
        let render_api = match api {
            Api::Vulkan => 2,
            Api::Dx12 => 1,
        };
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let paths = [wide.as_ptr()];
        // Feature ids: DLSS 0, Reflex 3, PCL 4, Frame Generation 1000.
        // Vulkan Reflex also needs the official bin/x64/NvLowLatencyVk.dll.
        // The SDK verifies this dependency; never bypass its signature checks.
        let mut features = Vec::new();
        if dlss {
            features.push(0u32);
        }
        let reflex_files =
            directory.join("sl.reflex.dll").is_file() && directory.join("sl.pcl.dll").is_file();
        let reflex = (reflex || framegen) && reflex_files;
        if reflex {
            features.push(3);
            features.push(4);
        }
        let framegen_files = directory.join("sl.dlss_g.dll").is_file()
            && directory.join("nvngx_dlssg.dll").is_file();
        let framegen_listed = framegen && framegen_files;
        if framegen_listed {
            features.push(1000);
        }
        if features.is_empty() {
            return Err("Streamline has no requested plugins".into());
        }
        let preferences = abi::Preferences {
            base: abi::Base::new(
                0x1ca10965,
                0xbf8e,
                0x432b,
                [0x8d, 0xa1, 0x67, 0x16, 0xd8, 0x79, 0xfb, 0x14],
                1,
            ),
            console: false,
            log_level: 0,
            paths: paths.as_ptr(),
            path_count: 1,
            log_path: std::ptr::null(),
            allocate: std::ptr::null(),
            release: std::ptr::null(),
            log: std::ptr::null(),
            flags: preference_flags(api),
            features: features.as_ptr(),
            feature_count: features.len() as u32,
            app_id: 0,
            engine: 0,
            engine_version: c"Horizon 0.1".as_ptr(),
            project_id: project_id.as_ptr(),
            render_api,
        };
        // Safety: ABI layout matches pinned headers; all pointers live through
        // slInit, whose contract copies preferences and strings internally.
        let result = unsafe { init(&preferences, abi::SDK_VERSION) };
        if result != 0 {
            return Err(format!("Streamline initialization failed ({result})"));
        }
        let mut dlss_loaded = false;
        // Safety: initialized SDK, valid writable bool and loaded function ABI.
        if dlss {
            let result = unsafe { loaded(0, &mut dlss_loaded) };
            if result != 0 || !dlss_loaded {
                unsafe {
                    shutdown();
                }
                return Err(format!("Streamline did not load DLSS ({result})"));
            }
        }
        if reflex {
            let (mut reflex_loaded, mut pcl_loaded) = (false, false);
            let reflex_code = unsafe { loaded(3, &mut reflex_loaded) };
            let pcl_code = unsafe { loaded(4, &mut pcl_loaded) };
            if reflex_code != 0 || pcl_code != 0 || !reflex_loaded || !pcl_loaded {
                unsafe {
                    shutdown();
                }
                return Err(format!(
                    "Streamline did not load Reflex ({reflex_code}/{pcl_code})"
                ));
            }
        }
        // A missing frame-generation plugin must not take DLSS or Reflex down.
        let mut framegen_ready = false;
        if framegen_listed {
            let mut loaded_fg = false;
            let code = unsafe { loaded(1000, &mut loaded_fg) };
            framegen_ready = code == 0 && loaded_fg;
        }
        Ok(Self {
            api,
            shutdown,
            requirements,
            supported,
            set_d3d_device,
            upgrade_interface,
            device: None,
            instance: None,
            presentation: Presentation::default(),
            frame: RenderFrame::default(),
            constants_failures: 0,
            directory,
            reflex: crate::reflex::Session::new(reflex),
            framegen: crate::frame_gen::Session::new(framegen_listed, framegen_ready),
            dlss_requested: dlss,
            _library: library,
            _session: guard,
        })
    }

    pub fn dlss_requested(&self) -> bool {
        self.dlss_requested
    }

    pub fn reflex_requested(&self) -> bool {
        self.reflex.requested()
    }

    /// Plugins are loaded and their functions were found after the device existed.
    pub fn reflex_loaded(&self) -> bool {
        self.reflex.loaded()
    }

    pub fn reflex_open(&self) -> bool {
        self.reflex.is_open()
    }

    pub fn set_reflex_mode(&mut self, mode: u32) {
        self.reflex.set_mode(mode);
    }

    /// Start the presented frame. Sleeps only when this token was not already
    /// slept at the previous present. `allow_sleep` is false offscreen.
    pub fn reflex_begin(&mut self, allow_sleep: bool) {
        if self.presentation_ready().is_ok() {
            self.reflex.begin(allow_sleep);
        }
    }

    pub fn reflex_end_simulation(&mut self) {
        self.reflex.end_simulation();
    }

    pub fn reflex_submit(&mut self, start: bool) {
        self.reflex.submit(start);
    }

    pub fn reflex_present(&mut self, start: bool) {
        self.reflex.present(start);
    }

    pub fn reflex_abandon_if_open(&mut self) {
        self.reflex.abandon_if_open();
    }

    /// Open once after Reflex's simulation begin, before any SR/FG work.
    pub fn begin_render_frame(&mut self) {
        self.frame.begin(self.reflex.token() as usize);
    }

    pub(crate) fn render_frame_active(&self) -> bool {
        self.frame.active
    }

    #[cfg(test)]
    pub(crate) fn check_null_frame_tags(&self, token: *const std::ffi::c_void) {
        type SetTag = unsafe extern "C" fn(
            *const std::ffi::c_void,
            *const crate::dlss_abi::Viewport,
            *const crate::dlss_abi::Tag,
            u32,
            *mut std::ffi::c_void,
        ) -> i32;
        let tags = [crate::frame_gen::null_tag(0), crate::frame_gen::null_tag(1)];
        // Null tags need no GPU copy/command list. This exercises the actual
        // SDK flag check and the paused-frame contract alongside SR local tags.
        let code = unsafe {
            let set = self._library.get::<SetTag>(b"slSetTagForFrame\0").unwrap();
            set(
                token,
                &crate::dlss_abi::Viewport::new(),
                tags.as_ptr(),
                2,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(code, 0, "SDK rejected null frame tags");
    }

    /// Errors invalidate generation for this frame; the next eligible frame
    /// resets both temporal consumers before building common constants.
    pub fn fail_render_frame(&mut self) {
        self.frame.failed = true;
        self.framegen.invalidate_history();
    }

    /// Retire acquire/encode early exits without Present and reset the next
    /// eligible frame. Surface recovery alone owns Off retries and backoff.
    pub fn end_render_frame(&mut self, rendered: bool) {
        self.frame = RenderFrame::default();
        if !rendered {
            self.reflex.abandon_if_open();
            self.framegen.invalidate_history();
        }
    }

    pub(crate) fn common_constants(
        &mut self,
        frame: crate::FrameConstants,
    ) -> Result<*mut std::ffi::c_void, String> {
        self.presentation_ready()?;
        // Safety: the runtime owns these pinned public ABI symbols and remains
        // loaded through all consumers of the returned token.
        let (allocate, set) = unsafe {
            (
                *self
                    ._library
                    .get::<crate::dlss_abi::NewToken>(b"slGetNewFrameToken\0")
                    .map_err(|e| e.to_string())?,
                *self
                    ._library
                    .get::<crate::dlss_abi::SetConstants>(b"slSetConstants\0")
                    .map_err(|e| e.to_string())?,
            )
        };
        let already_failed = self.frame.failed;
        let result = self
            .frame
            .constants(
                frame,
                || {
                    let mut token = std::ptr::null_mut();
                    let code = unsafe { allocate(&mut token, std::ptr::null()) };
                    if code != 0 || token.is_null() {
                        return Err(format!("Streamline frame token failed ({code})"));
                    }
                    Ok(token as usize)
                },
                |token, frame| {
                    let constants = crate::super_resolution::native_constants(frame);
                    let viewport = crate::dlss_abi::Viewport::new();
                    let code = unsafe { set(&constants, token as *mut _, &viewport) };
                    if code == 0 {
                        Ok(())
                    } else {
                        Err(format!("Streamline common constants failed ({code})"))
                    }
                },
            )
            .map(|token| token as *mut _);
        if result.is_ok() {
            self.constants_failures = 0;
        } else if !already_failed {
            self.constants_failures = self.constants_failures.saturating_add(1);
            if self.constants_failures == 3 {
                self.framegen
                    .fail("Common constants failed for three frames; restart required".into());
            }
        }
        result
    }

    pub fn reflex_line(&self) -> String {
        self.presentation.error.as_ref().map_or_else(
            || self.reflex.line(),
            |error| format!("Reflex: unavailable ({error})"),
        )
    }

    pub fn take_reflex_error(&mut self) -> Option<String> {
        self.reflex.take_error()
    }

    /// Mark simulation on the server thread only when the window has not
    /// already done it for this presented frame. Returns the token address.
    pub(crate) fn reflex_claim_server(&mut self) -> Option<usize> {
        self.presentation_ready().ok()?;
        self.reflex.claim_server()
    }

    pub(crate) fn reflex_finish_server(&mut self, token: usize) {
        self.reflex.finish_server(token);
    }

    /// Versions reported by the initialized DLSS plugin, not file names.
    pub fn dlss_versions(&self) -> Result<String, String> {
        let mut versions = abi::FeatureVersion {
            base: abi::Base::new(
                0x6d5b51f0,
                0x076b,
                0x486d,
                [0x99, 0x95, 0x5a, 0x56, 0x10, 0x43, 0xf5, 0xc1],
                1,
            ),
            streamline: abi::Version::default(),
            ngx: abi::Version::default(),
        };
        // Safety: initialized plugin, pinned public ABI and writable structure.
        let code = unsafe {
            let get = self
                ._library
                .get::<abi::GetFeatureVersion>(b"slGetFeatureVersion\0")
                .map_err(|e| e.to_string())?;
            get(0, &mut versions)
        };
        if code != 0 {
            return Err(format!("DLSS version query failed ({code})"));
        }
        let v = |v: &abi::Version| format!("{}.{}.{}", v.major, v.minor, v.build);
        Ok(format!(
            "DLSS {} / SL {}",
            v(&versions.ngx),
            v(&versions.streamline)
        ))
    }

    /// Creates a Vulkan instance through Streamline's official interposer, so
    /// its Vulkan extensions, hooks and presentation bookkeeping are installed.
    /// Keep this runtime until GPU work is drained, then drop it before the instance.
    pub fn create_instance(
        &mut self,
        desc: &wgpu::InstanceDescriptor,
    ) -> Result<wgpu::Instance, String> {
        if self.instance.is_some() {
            return Err("Streamline instance already created".into());
        }
        if self.api == Api::Dx12 {
            if desc.backends != wgpu::Backends::DX12 {
                return Err(
                    "Streamline DirectX 12 instance must use DirectX 12 exclusively".into(),
                );
            }
            // DirectX 12 does not use the display handle. wgpu creates the
            // device itself; Streamline sees it through slSetD3DDevice.
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: desc.backends,
                flags: desc.flags,
                memory_budget_thresholds: desc.memory_budget_thresholds,
                backend_options: desc.backend_options.clone(),
                display: None,
            });
            self.instance = Some(instance.clone());
            return Ok(instance);
        }
        if desc.backends != wgpu::Backends::VULKAN {
            return Err("Streamline instance must use Vulkan exclusively".into());
        }
        // Safety: same explicitly loaded, official Vulkan interposer as slInit.
        let entry = unsafe { ash::Entry::load_from(self.directory.join("sl.interposer.dll")) }
            .map_err(|e| e.to_string())?;
        let descriptor = wgpu::hal::InstanceDescriptor {
            name: "Horizon",
            flags: desc.flags,
            memory_budget_thresholds: desc.memory_budget_thresholds,
            backend_options: desc.backend_options.clone(),
            telemetry: None,
            display: None,
        };
        // Safety: interposer implements Vulkan loader ABI, and wgpu owns all
        // instance/device handles. Streamline initialization preceded this call.
        let native =
            unsafe { wgpu::hal::vulkan::Instance::init_with_entry(&descriptor, entry, None) }
                .map_err(|e| e.to_string())?;
        let instance = unsafe { wgpu::Instance::from_hal::<wgpu::hal::api::Vulkan>(native) };
        self.instance = Some(instance.clone());
        Ok(instance)
    }

    /// Register a live device created through the interposed instance.
    pub fn attach_device(&mut self, device: &wgpu::Device) -> Result<(), String> {
        if self.device.is_some() {
            return Err("Streamline device already registered".into());
        }
        if self.api == Api::Dx12 {
            // Safety: the guard keeps the device alive. The pointer is the
            // native ID3D12Device, which slSetD3DDevice copies from.
            let ptr = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or("DirectX 12 device missing")?
                .raw_device_ptr();
            if ptr.is_null() {
                return Err("DirectX 12 device missing".into());
            }
            let result = unsafe { (self.set_d3d_device)(ptr) };
            if result != 0 {
                return Err(format!("Streamline device registration failed ({result})"));
            }
            self.device = Some(device.clone());
            self.bind_features();
            return Ok(());
        }
        let instance = self
            .instance
            .as_ref()
            .ok_or("Streamline instance missing")?;
        // Safety: read-only guards keep all native handles alive.
        let (native, expected) = unsafe {
            (
                device.as_hal::<wgpu::hal::api::Vulkan>(),
                instance.as_hal::<wgpu::hal::api::Vulkan>(),
            )
        };
        let native = native.ok_or("Vulkan device missing")?;
        let expected = expected.ok_or("Vulkan instance missing")?;
        let instance = native.shared_instance().raw_instance().handle();
        if instance != expected.shared_instance().raw_instance().handle() {
            return Err("Device belongs to another Vulkan instance".into());
        }
        // vkCreateDevice came from the interposer: it already registered and
        // initialized the device. slSetVulkanInfo is for manual hooking only.
        self.device = Some(device.clone());
        self.bind_features();
        Ok(())
    }

    /// Feature functions exist only after the device is registered. A Reflex
    /// or frame-generation failure leaves the device usable.
    fn bind_features(&mut self) {
        if let Err(error) = self.reflex.bind(&self._library) {
            self.reflex.fail(error);
        }
        if let Err(error) = self.framegen.bind(&self._library) {
            self.framegen.fail(error);
        }
    }

    pub fn framegen_requested(&self) -> bool {
        self.framegen.requested()
    }

    pub fn framegen_loaded(&self) -> bool {
        self.framegen.loaded()
    }

    pub fn framegen_max_generated(&self) -> u32 {
        self.framegen.max_generated()
    }

    /// `generated` is the number of extra frames. Zero turns generation off.
    pub fn set_framegen_wanted(&mut self, generated: u32) {
        self.framegen.set_wanted(generated);
    }

    /// Checked Off before a surface/configuration/window mutation. The caller
    /// must then drain the presenting queue before changing GPU resources.
    pub fn suspend_framegen(&mut self) -> Result<(), String> {
        self.framegen.suspend()
    }

    /// A pending reset that can be applied to a genuine FG attempt. Blocked
    /// sessions retain the request without repeatedly resetting fallback SR/TAA.
    /// Read before building common camera constants, including SR.
    pub fn framegen_reset_pending(&self, output: [u32; 2]) -> bool {
        self.framegen.reset_ready(
            output,
            self.reflex.active(),
            self.presentation_ready().is_ok(),
        )
    }

    /// Tag depth and motion, then set this present's options. The token is the
    /// Reflex frame already open on this thread. Barriers stay on `encoder`.
    /// The returned buffer, when present, is only the raw tag copy: submit it
    /// after `encoder` and before Present.
    pub fn framegen_record(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        mut input: crate::frame_gen::Inputs<'_>,
    ) -> Result<Option<wgpu::CommandBuffer>, String> {
        if self.presentation_ready().is_err() || !self.framegen.loaded() {
            return Ok(None);
        }
        let Some(device) = self.device.clone() else {
            return Err("Frame Generation has no graphics device".into());
        };
        let dx12 = self.api == Api::Dx12;
        input.allow &= !self.frame.failed && self.reflex.active();
        let token = if input.allow {
            if let Some(constants) = input.constants {
                match self.common_constants(constants) {
                    Ok(token) => token,
                    Err(error) => {
                        self.fail_render_frame();
                        self.framegen.suspend()?;
                        self.framegen.note(error);
                        input.allow = false;
                        return self.framegen.record(
                            dx12,
                            self.frame.token as *mut _,
                            &device,
                            encoder,
                            input,
                        );
                    }
                }
            } else {
                std::ptr::null_mut()
            }
        } else {
            self.frame.token as *mut _
        };
        let result = self.framegen.record(dx12, token, &device, encoder, input);
        if let Err(error) = result {
            self.fail_render_frame();
            self.framegen.suspend()?;
            self.framegen.note(error);
            return Ok(None);
        }
        result
    }

    pub fn framegen_sample(&mut self) {
        if self.presentation_ready().is_ok() {
            self.framegen.sample();
        }
    }

    pub fn framegen_line(&self) -> String {
        self.presentation.error.as_ref().map_or_else(
            || self.framegen.line(),
            |error| format!("Frame Gen: unavailable ({error})"),
        )
    }

    pub fn take_framegen_error(&mut self) -> Option<String> {
        self.framegen.take_error()
    }

    pub(crate) fn registered_device(&self) -> Option<&wgpu::Device> {
        self.device.as_ref()
    }

    /// Checks the actual adapter before device creation. A successful result
    /// still requires the device to be registered and DLSS to be set up.
    pub fn check_adapter(&self, adapter: &wgpu::Adapter) -> Result<(), String> {
        self.check_feature_adapter(adapter, 0)
    }

    /// Confirm the requested presentation features on the actual registered
    /// device. Loaded DLLs and resolved functions alone do not prove support.
    /// A failure leaves normal rendering and independently supported SR usable.
    pub fn check_present_features(&mut self, adapter: &wgpu::Adapter) {
        if self.reflex.requested() {
            let result = self
                .check_feature_adapter(adapter, 3)
                .and_then(|()| self.check_feature_adapter(adapter, 4));
            if let Err(error) = result {
                self.reflex.fail(error);
            }
        }
        if self.framegen.requested() {
            let result = self.check_feature_adapter(adapter, 1000).and_then(|()| {
                if self.reflex.loaded() {
                    Ok(())
                } else {
                    Err("Frame Generation requires available Reflex".into())
                }
            });
            if let Err(error) = result {
                self.framegen.fail(error);
            }
        }
    }

    fn check_feature_adapter(&self, adapter: &wgpu::Adapter, feature: u32) -> Result<(), String> {
        let info = adapter.get_info();
        query_feature_support(
            self.api,
            info.backend,
            info.vendor,
            &info.name,
            feature,
            || {
                let base = abi::Base::new(
                    0x0677315f,
                    0xa746,
                    0x4492,
                    [0x9f, 0x42, 0xcb, 0x61, 0x42, 0xc9, 0xc3, 0xd4],
                    1,
                );
                // Safety: matching SDK ABI. The hal guard and adapter stay alive across
                // the call. A Vulkan physical device makes Streamline ignore the LUID.
                Ok(if self.api == Api::Dx12 {
                    let native = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() }
                        .ok_or("DirectX 12 adapter unavailable")?;
                    let luid = native
                        .adapter_luid()
                        .ok_or("DirectX 12 adapter LUID unavailable")?;
                    let adapter_info = abi::AdapterInfo {
                        base,
                        luid: luid.as_ptr(),
                        luid_size: 8,
                        physical_device: std::ptr::null_mut(),
                    };
                    unsafe { (self.supported)(feature, &adapter_info) }
                } else {
                    use ash::vk::Handle;
                    let native = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() }
                        .ok_or("Vulkan adapter unavailable")?;
                    let adapter_info = abi::AdapterInfo {
                        base,
                        luid: std::ptr::null(),
                        luid_size: 0,
                        physical_device: native.raw_physical_device().as_raw()
                            as *mut std::ffi::c_void,
                    };
                    unsafe { (self.supported)(feature, &adapter_info) }
                })
            },
        )
    }

    /// Install Streamline's DirectX 12 swapchain proxy so Present retires
    /// frame tokens. No-op on Vulkan, where the interposer already does this.
    /// Failure leaves the native swapchain in place.
    pub fn note_dx12_swapchain(&mut self, surface: &wgpu::Surface) -> Result<(), String> {
        if self.api != Api::Dx12 {
            return Ok(());
        }
        let upgrade = self.upgrade_interface;
        let result = (|| {
            // Safety: the guard and HAL's exclusive swapchain lock keep the
            // native interface alive. HAL remembers successful upgrades on the
            // owned chain itself, preserving that state across ResizeBuffers.
            let hal = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or("DirectX 12 surface missing")?;
            let mut error = "DX12 swapchain missing".to_owned();
            let ok = unsafe {
                hal.upgrade_swap_chain(|native| match upgrade_dx12_interface(native, upgrade) {
                    Ok(proxy) => proxy,
                    Err(reason) => {
                        error = reason;
                        std::ptr::null_mut()
                    }
                })
            };
            if ok { Ok(()) } else { Err(error) }
        })();
        let result = self.presentation.update(result);
        if result.is_err() {
            self.reflex.abandon_presentation();
        }
        result
    }

    /// Offscreen SR never requires a hook. Only an attempted, failed DX12
    /// presentation setup makes the token/Present contract unavailable.
    pub fn presentation_ready(&self) -> Result<(), String> {
        self.presentation.ready()
    }

    pub fn requirements(&self) -> Result<Requirements, String> {
        let mut req = abi::Requirements::new();
        // Safety: SDK initialized and req is the writable v2 ABI structure.
        let result = unsafe { (self.requirements)(0, &mut req) };
        if result != 0 {
            return Err(format!("DLSS requirements unavailable ({result})"));
        }
        Ok(Requirements {
            instance_extensions: names(req.instance_extensions, req.instance_extension_count)?,
            device_extensions: names(req.device_extensions, req.device_extension_count)?,
            features12: names(req.features12, req.feature12_count)?,
            features13: names(req.features13, req.feature13_count)?,
            compute_queues: req.compute_queues,
            graphics_queues: req.graphics_queues,
        })
    }
}

/// Failed hooks are recoverable without discarding any native SDK owners.
#[derive(Default)]
struct Presentation {
    error: Option<String>,
}
impl Presentation {
    fn update(&mut self, result: Result<(), String>) -> Result<(), String> {
        self.error = result.as_ref().err().cloned();
        result
    }
    fn ready(&self) -> Result<(), String> {
        self.error.clone().map_or(Ok(()), Err)
    }
}

// Safety: called only under the HAL chain's exclusive lock. Success transfers
// the SDK's owned proxy reference to HAL; failure transfers no ownership.
unsafe fn upgrade_dx12_interface(
    native: *mut std::ffi::c_void,
    upgrade: abi::UpgradeInterface,
) -> Result<*mut std::ffi::c_void, String> {
    let mut proxy = native;
    let code = unsafe { upgrade(&mut proxy) };
    if code == 0 && !proxy.is_null() {
        Ok(proxy)
    } else {
        Err(format!(
            "DX12 presentation hook failed (slUpgradeInterface: {code}, null proxy: {})",
            proxy.is_null()
        ))
    }
}

/// Reflex and PCL support is defined by the SDK, independently of DLSS/RTX.
/// Keep API/session identity checks common, then query the actual native adapter.
fn query_feature_support(
    api: Api,
    backend: wgpu::Backend,
    vendor: u32,
    name: &str,
    feature: u32,
    query: impl FnOnce() -> Result<i32, String>,
) -> Result<(), String> {
    let matches_session = matches!(
        (api, backend),
        (Api::Vulkan, wgpu::Backend::Vulkan) | (Api::Dx12, wgpu::Backend::Dx12)
    );
    if !matches_session {
        return Err("Streamline graphics API does not match this adapter".into());
    }
    if matches!(feature, 0 | 1000) && !crate::eligible(true, vendor, name) {
        return Err("DLSS requires Vulkan or DirectX 12 on an NVIDIA RTX adapter".into());
    }
    let result = query()?;
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "Streamline feature {feature} unsupported on this adapter ({result})"
        ))
    }
}

fn preference_flags(api: Api) -> u64 {
    abi::DISABLE_CL_STATE_TRACKING
        | abi::USE_FRAME_BASED_RESOURCE_TAGGING
        | if api == Api::Dx12 {
            abi::USE_MANUAL_HOOKING
        } else {
            0
        }
}

fn valid_project_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

use std::os::windows::ffi::OsStrExt;
fn names(values: *const *const std::ffi::c_char, count: u32) -> Result<Vec<String>, String> {
    if count > 1024 || (count != 0 && values.is_null()) {
        return Err("Invalid Streamline requirements".into());
    }
    (0..count as usize)
        .map(|i| {
            // Safety: initialized SDK owns this array and strings until shutdown.
            let value = unsafe { *values.add(i) };
            if value.is_null() {
                return Err("Null Streamline requirement".into());
            }
            Ok(unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned())
        })
        .collect()
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(device) = &self.device {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        // Safety: exclusive process-global owner, library still loaded.
        unsafe {
            (self.shutdown)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtx_reflex_and_pcl_reach_sdk_while_dlss_retains_its_rtx_gate() {
        for (api, backend) in [
            (Api::Vulkan, wgpu::Backend::Vulkan),
            (Api::Dx12, wgpu::Backend::Dx12),
        ] {
            for feature in [3, 4] {
                let calls = std::cell::Cell::new(0);
                for code in [0, 42] {
                    let result = query_feature_support(
                        api,
                        backend,
                        0x10de,
                        "GeForce GTX 1080",
                        feature,
                        || {
                            calls.set(calls.get() + 1);
                            Ok(code)
                        },
                    );
                    assert_eq!(result.is_ok(), code == 0);
                }
                assert_eq!(calls.get(), 2); // SDK truth, including its rejection
            }
            for feature in [0, 1000] {
                assert!(
                    query_feature_support(
                        api,
                        backend,
                        0x10de,
                        "GeForce GTX 1080",
                        feature,
                        || panic!("RTX gate bypassed")
                    )
                    .is_err()
                );
            }
            let other = if api == Api::Vulkan {
                wgpu::Backend::Dx12
            } else {
                wgpu::Backend::Vulkan
            };
            assert!(
                query_feature_support(api, other, 0x10de, "GeForce GTX 1080", 3, || panic!(
                    "API identity bypassed"
                ))
                .is_err()
            );
            // PCL is cross-vendor; it must also defer to the SDK.
            assert!(query_feature_support(api, backend, 0x1002, "Radeon", 4, || Ok(0)).is_ok());
        }
    }

    #[test]
    fn preferences_enable_frame_tags_on_both_apis_without_ota_or_bypasses() {
        assert_eq!(preference_flags(Api::Vulkan), 0x81);
        assert_eq!(preference_flags(Api::Dx12), 0x85);
    }

    fn constants(reset: bool) -> crate::FrameConstants {
        crate::FrameConstants {
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
            reset,
        }
    }

    #[test]
    fn sr_and_fg_share_one_submission_and_token_across_normal_and_reset_frames() {
        let calls = std::cell::RefCell::new(Vec::new());
        let mut frame = RenderFrame::default();
        for (index, reset) in [true, false, false, true, false].into_iter().enumerate() {
            // Reflex token if present; SR-only frames allocate lazily instead.
            let token = index + 10;
            frame.begin(if index % 2 == 0 { token } else { 0 });
            let value = constants(reset);
            let sr = frame
                .constants(
                    value,
                    || Ok(token),
                    |t, c| {
                        calls.borrow_mut().push((t, c.reset));
                        Ok(())
                    },
                )
                .unwrap();
            let fg = frame
                .constants(
                    value,
                    || panic!("second token"),
                    |_, _| panic!("second constants submission"),
                )
                .unwrap();
            assert_eq!((sr, fg), (token, token));
        }
        assert_eq!(
            *calls.borrow(),
            [
                (10, true),
                (11, false),
                (12, false),
                (13, true),
                (14, false)
            ]
        );
    }

    #[test]
    fn failed_sr_constants_and_changed_constants_block_fg_until_a_new_frame() {
        let mut frame = RenderFrame::default();
        frame.begin(42);
        assert!(
            frame
                .constants(
                    constants(false),
                    || panic!(),
                    |_, _| Err("SDK error".into())
                )
                .is_err()
        );
        assert!(
            frame
                .constants(
                    constants(false),
                    || panic!(),
                    |_, _| panic!("FG after SR error")
                )
                .is_err()
        );
        frame.begin(43);
        frame
            .constants(constants(false), || panic!(), |_, _| Ok(()))
            .unwrap();
        assert!(
            frame
                .constants(
                    constants(true),
                    || panic!(),
                    |_, _| panic!("mutated reset reached SDK")
                )
                .is_err()
        );
        frame.begin(44);
        assert_eq!(
            frame
                .constants(constants(true), || panic!(), |_, _| Ok(()))
                .unwrap(),
            44
        );
        frame = RenderFrame::default(); // end / skipped frame
        assert!(
            frame
                .constants(constants(false), || panic!(), |_, _| panic!())
                .is_err()
        );
        frame.begin(45);
        let mut invalid = constants(false);
        invalid.far = f32::NAN;
        assert!(
            frame
                .constants(invalid, || panic!(), |_, _| panic!("NaN reached SDK"))
                .is_err()
        );
    }

    unsafe extern "C" fn mock_upgrade(ptr: *mut *mut std::ffi::c_void) -> i32 {
        match unsafe { *ptr } as usize {
            1 => {
                unsafe { *ptr = 2usize as *mut _ };
                0
            }
            3 => {
                unsafe { *ptr = std::ptr::null_mut() };
                0
            }
            _ => 42,
        }
    }

    #[test]
    fn hook_failure_blocks_consumers_and_successful_new_chain_recovers() {
        let mut presentation = Presentation::default();
        let mut framegen = crate::frame_gen::Session::new(true, true);
        framegen.set_wanted(1);
        framegen.suspend().unwrap();
        assert!(presentation.ready().is_ok()); // offscreen / no hook attempted
        let error = presentation
            .update(unsafe { upgrade_dx12_interface(2usize as *mut _, mock_upgrade).map(|_| ()) })
            .unwrap_err();
        assert!(error.contains("42"));
        let mut called = false;
        let result = presentation.ready().map(|()| called = true);
        assert_eq!(result.unwrap_err(), error);
        assert!(!called); // token allocation/evaluation must remain gated
        for _ in 0..4 {
            assert!(presentation.ready().is_err());
            assert!(framegen.reset_pending());
        }
        presentation
            .update(unsafe {
                upgrade_dx12_interface(std::ptr::dangling_mut(), mock_upgrade).map(|proxy| {
                    assert_eq!(proxy as usize, 2);
                })
            })
            .unwrap();
        assert!(presentation.ready().is_ok());
        assert!(framegen.reset_pending()); // first recovered FG frame still needs reset
    }

    #[test]
    fn successful_sdk_result_with_null_proxy_is_unavailable() {
        let result = unsafe { upgrade_dx12_interface(3usize as *mut _, mock_upgrade) };
        assert!(result.unwrap_err().contains("null proxy: true"));
    }
    #[test]
    fn absent_sdk_and_invalid_configuration_return_errors() {
        assert!(Runtime::load(Path::new("relative"), "invalid").is_err());
        let directory = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(Runtime::load(&directory, "invalid").is_err());
        assert!(Runtime::load(&directory, "00a6bdb2-6386-4e37-8ff9-dcda4642628a").is_err());
        assert!(valid_project_id("00a6bdb2-6386-4e37-8ff9-dcda4642628a"));
    }

    fn present_feature_support(api: Api) {
        let path = std::env::var_os("HORIZON_STREAMLINE_SDK").expect("SDK path");
        let mut runtime = Runtime::load_with(
            Path::new(&path),
            "00a6bdb2-6386-4e37-8ff9-dcda4642628a",
            api,
            false,
            true,
            true,
        )
        .unwrap();
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = if api == Api::Dx12 {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::VULKAN
        };
        let instance = runtime.create_instance(&desc).unwrap();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, _) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        runtime.attach_device(&device).unwrap();
        runtime.check_present_features(&adapter);
        println!("{api:?}: {}", adapter.get_info().name);
        assert!(runtime.reflex_loaded(), "{:?}", runtime.take_reflex_error());
        assert!(
            runtime.framegen_loaded(),
            "{:?}",
            runtime.take_framegen_error()
        );
        for mode in [1, 2, 0] {
            runtime.set_reflex_mode(mode);
            assert_eq!(runtime.take_reflex_error(), None);
        }
        runtime.suspend_framegen().unwrap();
        drop(runtime);
    }

    #[test]
    #[ignore = "requires the official SDK, Vulkan Reflex DLL, and RTX FG device"]
    fn official_sdk_confirms_vulkan_present_features() {
        present_feature_support(Api::Vulkan);
    }

    #[test]
    #[ignore = "requires the official SDK and an RTX DirectX 12 FG device"]
    fn official_sdk_confirms_dx12_present_features() {
        present_feature_support(Api::Dx12);
    }

    #[test]
    #[ignore = "requires an explicitly selected official NVIDIA SDK directory"]
    fn official_sdk_reports_requirements() {
        let path = std::env::var_os("HORIZON_STREAMLINE_SDK").expect("SDK path");
        let mut runtime = Runtime::load(Path::new(&path), "00a6bdb2-6386-4e37-8ff9-dcda4642628a")
            .expect("Streamline startup");
        let requirements = runtime.requirements().expect("DLSS requirements");
        println!("{requirements:?}");
        assert!(!requirements.device_extensions.is_empty());
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        let instance = runtime
            .create_instance(&desc)
            .expect("Streamline Vulkan instance");
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("Vulkan adapter");
        println!("Adapter: {}", adapter.get_info().name);
        runtime
            .check_adapter(&adapter)
            .expect("DLSS physical-adapter support");
        let (device, _) = pollster::block_on(adapter.request_device(&Default::default()))
            .expect("Interposed Vulkan device");
        runtime
            .attach_device(&device)
            .expect("Streamline device registration");
        runtime
            .check_adapter(&adapter)
            .expect("DLSS support after device registration");
        drop(runtime);
    }
}
