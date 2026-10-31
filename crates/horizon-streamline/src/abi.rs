/*
* Copyright (c) 2022-2024 NVIDIA CORPORATION. All rights reserved
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
// Rust layout translation of Streamline 2.14.16 public MIT headers.
// Pinned source: NVIDIA-RTX/Streamline 625db07d4bca5abc67d781c42af7e7fb0761f1b6.
use std::ffi::{c_char, c_void};
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid {
    pub a: u32,
    pub b: u16,
    pub c: u16,
    pub d: [u8; 8],
}
#[repr(C)]
pub struct Base {
    pub next: *mut c_void,
    pub guid: Guid,
    pub version: usize,
}
impl Base {
    pub fn new(a: u32, b: u16, c: u16, d: [u8; 8], version: usize) -> Self {
        Self {
            next: std::ptr::null_mut(),
            guid: Guid { a, b, c, d },
            version,
        }
    }
}
#[repr(C)]
pub struct Preferences {
    pub base: Base,
    pub console: bool,
    pub log_level: u32,
    pub paths: *const *const u16,
    pub path_count: u32,
    pub log_path: *const u16,
    pub allocate: *const c_void,
    pub release: *const c_void,
    pub log: *const c_void,
    pub flags: u64,
    pub features: *const u32,
    pub feature_count: u32,
    pub app_id: u32,
    pub engine: u32,
    pub engine_version: *const c_char,
    pub project_id: *const c_char,
    pub render_api: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
}
#[repr(C)]
pub struct Requirements {
    pub base: Base,
    pub flags: u32,
    pub cpu_threads: u32,
    pub viewports: u32,
    pub tag_count: u32,
    pub tags: *const u32,
    pub os_detected: Version,
    pub os_required: Version,
    pub driver_detected: Version,
    pub driver_required: Version,
    pub compute_queues: u32,
    pub graphics_queues: u32,
    pub device_extension_count: u32,
    pub device_extensions: *const *const c_char,
    pub instance_extension_count: u32,
    pub instance_extensions: *const *const c_char,
    pub feature12_count: u32,
    pub features12: *const *const c_char,
    pub feature13_count: u32,
    pub features13: *const *const c_char,
    pub optical_flow_queues: u32,
}
impl Requirements {
    pub fn new() -> Self {
        Self {
            base: Base::new(
                0x66714097,
                0xac6d,
                0x4bc6,
                [0x89, 0x15, 0x1e, 0x0f, 0x55, 0xa6, 0xb6, 0x1f],
                2,
            ),
            flags: 0,
            cpu_threads: 0,
            viewports: 0,
            tag_count: 0,
            tags: std::ptr::null(),
            os_detected: Version::default(),
            os_required: Version::default(),
            driver_detected: Version::default(),
            driver_required: Version::default(),
            compute_queues: 0,
            graphics_queues: 0,
            device_extension_count: 0,
            device_extensions: std::ptr::null(),
            instance_extension_count: 0,
            instance_extensions: std::ptr::null(),
            feature12_count: 0,
            features12: std::ptr::null(),
            feature13_count: 0,
            features13: std::ptr::null(),
            optical_flow_queues: 0,
        }
    }
}
pub type Init = unsafe extern "C" fn(*const Preferences, u64) -> i32;
pub type Shutdown = unsafe extern "C" fn() -> i32;
pub type IsLoaded = unsafe extern "C" fn(u32, *mut bool) -> i32;
#[repr(C)]
pub struct AdapterInfo {
    pub base: Base,
    pub luid: *const u8,
    pub luid_size: u32,
    pub physical_device: *mut c_void,
}
pub type IsSupported = unsafe extern "C" fn(u32, *const AdapterInfo) -> i32;
pub type GetRequirements = unsafe extern "C" fn(u32, *mut Requirements) -> i32;
pub const SDK_VERSION: u64 = (2u64 << 48) | (14u64 << 32) | (16u64 << 16) | 0xfedc;

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};
    #[test]
    fn layouts_match_official_sdk_compiled_with_msvc_x64() {
        // Independently measured from the pinned C++ headers with MSVC.
        assert_eq!(size_of::<Base>(), 32);
        assert_eq!(size_of::<Preferences>(), 144);
        assert_eq!(size_of::<Requirements>(), 184);
        assert_eq!(offset_of!(Preferences, console), 32);
        assert_eq!(offset_of!(Preferences, flags), 88);
        assert_eq!(offset_of!(Preferences, render_api), 136);
        assert_eq!(offset_of!(Requirements, device_extensions), 120);
        assert_eq!(offset_of!(Requirements, features13), 168);
    }
}

#[repr(C)]
pub struct FeatureVersion {
    pub base: Base,
    pub streamline: Version,
    pub ngx: Version,
}
pub type GetFeatureVersion = unsafe extern "C" fn(u32, *mut FeatureVersion) -> i32;
