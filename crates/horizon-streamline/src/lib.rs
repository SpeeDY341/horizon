//! Streamline's native boundary. No SDK binaries or game assets are bundled.
//! Loading and initialization are explicit; ordinary game startup does neither.
#[cfg(target_os = "windows")]
mod abi;
#[cfg(target_os = "windows")]
mod runtime;
#[cfg(target_os = "windows")]
pub use runtime::{Api, Requirements, Runtime};

/// Cheap prefilter only. `supported_api` is true for Vulkan and DirectX 12.
/// Streamline must still confirm DLSS on that adapter; a name match is not
/// that confirmation.
pub fn eligible(supported_api: bool, vendor: u32, name: &str) -> bool {
    cfg!(target_os = "windows")
        && supported_api
        && vendor == 0x10de
        && name.to_ascii_uppercase().contains("RTX")
}

#[cfg(target_os = "windows")]
mod dlss_abi;
#[cfg(target_os = "windows")]
mod super_resolution;
#[cfg(target_os = "windows")]
pub use super_resolution::{FrameConstants, SuperResolution};

#[cfg(target_os = "windows")]
mod evaluate;

#[cfg(test)]
mod tests {
    #[test]
    fn other_backends_and_devices_are_rejected() {
        assert!(!super::eligible(false, 0x10de, "NVIDIA GeForce RTX 5090"));
        assert!(!super::eligible(true, 0x1002, "Radeon"));
        assert!(!super::eligible(true, 0x10de, "GeForce GTX 1080"));
    }
}

#[cfg(target_os = "windows")]
mod monitor;
#[cfg(target_os = "windows")]
pub use monitor::Performance;

#[cfg(target_os = "windows")]
mod frame_gen;
#[cfg(target_os = "windows")]
pub use frame_gen::Inputs;
#[cfg(target_os = "windows")]
mod reflex;
#[cfg(target_os = "windows")]
pub use reflex::{ReflexServerTick, install as install_reflex, reflex_server_tick};

/// No Reflex session exists off Windows. The server tick runs unchanged.
#[cfg(not(target_os = "windows"))]
pub struct ReflexServerTick;

#[cfg(not(target_os = "windows"))]
impl Drop for ReflexServerTick {
    fn drop(&mut self) {}
}

#[cfg(not(target_os = "windows"))]
pub fn reflex_server_tick() -> ReflexServerTick {
    ReflexServerTick
}
