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
    /// Proxy swapchain last installed. A second upgrade of the same pointer is
    /// rejected when manual hooking is on, and means the proxy is still in place.
    dx12_swapchain: usize,
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
        // PreferenceFlags: eDisableCLStateTracking = 1, eUseManualHooking = 4.
        let (render_api, flags) = match api {
            Api::Vulkan => (2, 1),
            Api::Dx12 => (1, 1 | 4),
        };
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let paths = [wide.as_ptr()];
        // Feature ids: DLSS 0, Reflex 3, PCL 4, Frame Generation 1000.
        // Reflex's own latency DLL is unsigned and is never placed beside these.
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
            flags,
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
            dx12_swapchain: 0,
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
        self.reflex.begin(allow_sleep);
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

    /// The token of the frame on screen, or null when Reflex is not marking it.
    /// DLSS must reuse this token instead of allocating another one.
    pub(crate) fn frame_token(&self) -> *mut std::ffi::c_void {
        self.reflex.token()
    }

    pub fn reflex_line(&self) -> String {
        self.reflex.line()
    }

    pub fn take_reflex_error(&mut self) -> Option<String> {
        self.reflex.take_error()
    }

    /// Mark simulation on the server thread only when the window has not
    /// already done it for this presented frame. Returns the token address.
    pub(crate) fn reflex_claim_server(&mut self) -> Option<usize> {
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

    /// `generated` is the number of extra frames. Zero turns generation off.
    pub fn set_framegen_wanted(&mut self, generated: u32) {
        self.framegen.set_wanted(generated);
    }

    /// Tag depth and motion, then set this present's options. The token is the
    /// Reflex frame already open on this thread. Barriers stay on `encoder`.
    /// The returned buffer, when present, is only the raw tag copy: submit it
    /// after `encoder` and before Present.
    pub fn framegen_record(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: crate::frame_gen::Inputs<'_>,
    ) -> Result<Option<wgpu::CommandBuffer>, String> {
        let Some(device) = self.device.clone() else {
            return Err("Frame Generation has no graphics device".into());
        };
        let dx12 = self.api == Api::Dx12;
        let token = self.frame_token();
        self.framegen.record(dx12, token, &device, encoder, input)
    }

    pub fn framegen_sample(&mut self) {
        self.framegen.sample();
    }

    pub fn framegen_line(&self) -> String {
        self.framegen.line()
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
        let info = adapter.get_info();
        let supported = matches!(info.backend, wgpu::Backend::Vulkan | wgpu::Backend::Dx12);
        if !crate::eligible(supported, info.vendor, &info.name) {
            return Err("DLSS requires Vulkan or DirectX 12 on an NVIDIA RTX adapter".into());
        }
        let matches_session = match self.api {
            Api::Vulkan => info.backend == wgpu::Backend::Vulkan,
            Api::Dx12 => info.backend == wgpu::Backend::Dx12,
        };
        if !matches_session {
            return Err("DLSS graphics API does not match this adapter".into());
        }
        let base = abi::Base::new(
            0x0677315f,
            0xa746,
            0x4492,
            [0x9f, 0x42, 0xcb, 0x61, 0x42, 0xc9, 0xc3, 0xd4],
            1,
        );
        // Safety: matching SDK ABI. The hal guard and adapter stay alive across
        // the call. A Vulkan physical device makes Streamline ignore the LUID.
        let result = if self.api == Api::Dx12 {
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
            unsafe { (self.supported)(0, &adapter_info) }
        } else {
            use ash::vk::Handle;
            let native = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("Vulkan adapter unavailable")?;
            let adapter_info = abi::AdapterInfo {
                base,
                luid: std::ptr::null(),
                luid_size: 0,
                physical_device: native.raw_physical_device().as_raw() as *mut std::ffi::c_void,
            };
            unsafe { (self.supported)(0, &adapter_info) }
        };
        if result == 0 {
            Ok(())
        } else {
            Err(format!("DLSS unsupported on this adapter ({result})"))
        }
    }

    /// Install Streamline's DirectX 12 swapchain proxy so Present retires
    /// frame tokens. No-op on Vulkan, where the interposer already does this.
    /// Failure leaves the native swapchain in place.
    pub fn note_dx12_swapchain(&mut self, surface: &wgpu::Surface) -> Result<(), String> {
        if self.api != Api::Dx12 {
            return Ok(());
        }
        let upgrade = self.upgrade_interface;
        let known = self.dx12_swapchain;
        // Safety: the surface guard keeps the hal surface alive. The callback
        // only calls slUpgradeInterface and does not record commands.
        let hal = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }
            .ok_or("DirectX 12 surface missing")?;
        let mut installed = 0usize;
        let ok = unsafe {
            hal.upgrade_swap_chain(|native| {
                let mut ptr = native;
                let code = upgrade(&mut ptr);
                if code == 0 && !ptr.is_null() {
                    installed = ptr as usize;
                    return ptr;
                }
                // Manual hooking rejects a second upgrade of a live proxy and
                // leaves the pointer unchanged.
                if !native.is_null() && native as usize == known {
                    installed = known;
                    return native;
                }
                std::ptr::null_mut()
            })
        };
        if ok {
            self.dx12_swapchain = installed;
            Ok(())
        } else {
            Err("Streamline did not hook the DirectX 12 swapchain".into())
        }
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
