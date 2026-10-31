use crate::abi;
use std::{
    ffi::{CStr, CString},
    path::Path,
    sync::{Mutex, MutexGuard},
};

// Streamline is process-global. Initialization/shutdown must never overlap,
// including two renderer startups or resource reloads.
static SESSION: Mutex<()> = Mutex::new(());

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
    _library: libloading::os::windows::Library,
    _session: MutexGuard<'static, ()>,
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
        let project_id = CString::new(project_id).map_err(|e| e.to_string())?;
        let guard = SESSION
            .try_lock()
            .map_err(|_| "Streamline is already in use")?;
        let path = directory.join("sl.interposer.dll");
        // Safety: explicit library path, restricted dependency search, only the
        // pinned public ABI is called; library remains alive until shutdown.
        let library = unsafe {
            libloading::os::windows::Library::load_with_flags(&path, 0x00000100 | 0x00000800)
        }
        .map_err(|e| e.to_string())?;
        // Resolve every required symbol before initialization, so a partial
        // incompatible DLL never leaves an initialized process-global SDK.
        let (init, shutdown, requirements) = unsafe {
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
        Ok(Self {
            shutdown,
            requirements,
            _library: library,
            _session: guard,
        })
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
}
