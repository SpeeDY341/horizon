//! Streamline's native boundary. No SDK binaries or game assets are bundled.
//! Loading and initialization are explicit; ordinary game startup does neither.
#[cfg(target_os = "windows")]
mod abi;
#[cfg(target_os = "windows")]
mod runtime;
#[cfg(target_os = "windows")]
pub use runtime::{Requirements, Runtime};

/// Cheap prefilter only. Streamline must still confirm DLSS support on the
/// actual Vulkan physical device; a vendor/name match is not that confirmation.
pub fn eligible(vulkan: bool, vendor: u32, name: &str) -> bool {
    cfg!(target_os = "windows")
        && vulkan
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
