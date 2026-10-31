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

#[derive(Debug)]
pub struct Requirements {
    pub instance_extensions: Vec<String>,
    pub device_extensions: Vec<String>,
    pub features12: Vec<String>,
    pub features13: Vec<String>,
    pub compute_queues: u32,
    pub graphics_queues: u32,
}

/// Initialized SDK, owned by the render thread. Must remain loaded throughout Vulkan use
/// and be shut down after draining GPU work, before device destruction.
/// This currently provides startup requirements, not a DLSS evaluation path.
pub struct Runtime {
    shutdown: abi::Shutdown,
    requirements: abi::GetRequirements,
    supported: abi::IsSupported,
    device: Option<wgpu::Device>,
    instance: Option<wgpu::Instance>,
    directory: std::path::PathBuf,
    pub(crate) _library: libloading::os::windows::Library,
    _session: Lease,
}

impl Runtime {
    /// Initialize from an explicitly selected SDK directory. Only the official
    /// production SDK belongs here. No PATH/current-directory DLL lookup.
    pub fn load(directory: &Path, project_id: &str) -> Result<Self, String> {
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
        let (init, shutdown, requirements, loaded, supported) = unsafe {
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
            )
        };
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let paths = [wide.as_ptr()];
        let features = [0u32]; // Super Resolution only.
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
            flags: 1, // No OTA or unrequested features; interposer manages Vulkan hooks.
            features: features.as_ptr(),
            feature_count: 1,
            app_id: 0,
            engine: 0,
            engine_version: c"Horizon 0.1".as_ptr(),
            project_id: project_id.as_ptr(),
            render_api: 2,
        };
        // Safety: ABI layout matches pinned headers; all pointers live through
        // slInit, whose contract copies preferences and strings internally.
        let result = unsafe { init(&preferences, abi::SDK_VERSION) };
        if result != 0 {
            return Err(format!("Streamline initialization failed ({result})"));
        }
        let mut dlss_loaded = false;
        // Safety: initialized SDK, valid writable bool and loaded function ABI.
        let result = unsafe { loaded(0, &mut dlss_loaded) };
        if result != 0 || !dlss_loaded {
            unsafe {
                shutdown();
            }
            return Err(format!("Streamline did not load DLSS ({result})"));
        }
        Ok(Self {
            shutdown,
            requirements,
            supported,
            device: None,
            instance: None,
            directory,
            _library: library,
            _session: guard,
        })
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
        Ok(())
    }

    /// Checks the actual physical adapter before device creation. A successful
    /// result still requires a correctly created Vulkan device and DLSS setup.
    pub fn check_adapter(&self, adapter: &wgpu::Adapter) -> Result<(), String> {
        use ash::vk::Handle;
        let info = adapter.get_info();
        if !crate::eligible(
            info.backend == wgpu::Backend::Vulkan,
            info.vendor,
            &info.name,
        ) {
            return Err("DLSS requires Vulkan on an NVIDIA RTX adapter".into());
        }
        // Safety: read-only native access; the guard and adapter keep it alive.
        let native = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() }
            .ok_or("Vulkan adapter unavailable")?;
        let info = abi::AdapterInfo {
            base: abi::Base::new(
                0x0677315f,
                0xa746,
                0x4492,
                [0x9f, 0x42, 0xcb, 0x61, 0x42, 0xc9, 0xc3, 0xd4],
                1,
            ),
            luid: std::ptr::null(),
            luid_size: 0,
            physical_device: native.raw_physical_device().as_raw() as *mut std::ffi::c_void,
        };
        // Safety: matching SDK ABI, live physical device and initialized SDK.
        let result = unsafe { (self.supported)(0, &info) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!("DLSS unsupported on this adapter ({result})"))
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
