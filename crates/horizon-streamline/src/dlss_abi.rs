/*
* Copyright (c) 2022-2024 NVIDIA CORPORATION. All rights reserved
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
// Adapted from MIT Streamline 2.14.16 public headers; see abi.rs for pin.
use crate::abi::Base;
use std::ffi::c_void;
#[repr(C)]
pub struct Viewport {
    pub base: Base,
    pub id: u32,
}
impl Viewport {
    pub fn new() -> Self {
        Self {
            base: Base::new(
                0x171b6435,
                0x9b3c,
                0x4fc8,
                [0x99, 0x94, 0xfb, 0xe5, 0x25, 0x69, 0xaa, 0xa4],
                1,
            ),
            id: 0,
        }
    }
}
#[repr(C)]
pub struct Options {
    pub base: Base,
    pub mode: u32,
    pub width: u32,
    pub height: u32,
    pub sharpness: f32,
    pub pre_exposure: f32,
    pub exposure: f32,
    pub hdr: u8,
    pub invert_x: u8,
    pub invert_y: u8,
    pub presets: [u32; 6],
    pub auto_exposure: u8,
    pub alpha: u8,
}
impl Options {
    pub fn new(mode: u32, output: [u32; 2]) -> Self {
        Self {
            base: Base::new(
                0x6ac826e4,
                0x4c61,
                0x4101,
                [0xa9, 0x2d, 0x63, 0x8d, 0x42, 0x10, 0x57, 0xb8],
                3,
            ),
            mode,
            width: output[0],
            height: output[1],
            sharpness: 0.0,
            pre_exposure: 1.0,
            exposure: 1.0,
            hdr: 0,
            invert_x: 0,
            invert_y: 0,
            presets: [0; 6],
            // NVIDIA's default. Alpha upscaling is a separate, slower path and
            // the world is copied as opaque colour; the HUD is drawn afterwards.
            auto_exposure: 1,
            alpha: 0,
        }
    }
}
#[repr(C)]
pub struct Optimal {
    pub base: Base,
    pub width: u32,
    pub height: u32,
    pub sharpness: f32,
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
}
impl Optimal {
    pub fn new() -> Self {
        Self {
            base: Base::new(
                0xef1d0957,
                0xfd58,
                0x4df7,
                [0xb5, 0x04, 0x8b, 0x69, 0xd8, 0xaa, 0x6b, 0x76],
                1,
            ),
            width: 0,
            height: 0,
            sharpness: 0.0,
            min_width: 0,
            min_height: 0,
            max_width: 0,
            max_height: 0,
        }
    }
}
#[repr(C)]
pub struct Constants {
    pub base: Base,
    pub matrices: [[[f32; 4]; 4]; 5],
    pub jitter: [f32; 2],
    pub motion_scale: [f32; 2],
    pub pinhole: [f32; 2],
    pub camera: [[f32; 3]; 4],
    pub near: f32,
    pub far: f32,
    pub fov: f32,
    pub aspect: f32,
    pub invalid_motion: f32,
    pub flags: [u8; 7],
    pub separation: f32,
}
#[repr(C)]
pub struct Resource {
    pub base: Base,
    pub kind: u8,
    pub native: *mut c_void,
    pub memory: *mut c_void,
    pub view: *mut c_void,
    pub state: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub mips: u32,
    pub layers: u32,
    pub gpu_address: u64,
    pub flags: u32,
    pub usage: u32,
    pub internal_flags: u16,
    pub reserved: u16,
}
#[repr(C)]
pub struct Tag {
    pub base: Base,
    pub resource: *mut Resource,
    pub kind: u32,
    pub lifecycle: u32,
    pub extent: [u32; 4],
}
pub type GetFunction = unsafe extern "C" fn(u32, *const std::ffi::c_char, *mut *mut c_void) -> i32;
pub type GetOptimal = unsafe extern "C" fn(*const Options, *mut Optimal) -> i32;
pub type SetOptions = unsafe extern "C" fn(*const Viewport, *const Options) -> i32;
pub type NewToken = unsafe extern "C" fn(*mut *mut c_void, *const u32) -> i32;
pub type SetConstants =
    unsafe extern "C" fn(*const Constants, *const c_void, *const Viewport) -> i32;
pub type Evaluate =
    unsafe extern "C" fn(u32, *const c_void, *const *const Base, u32, *mut c_void) -> i32;
