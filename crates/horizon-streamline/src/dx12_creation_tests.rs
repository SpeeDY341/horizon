//! Explicit native creation regression; does not assert focused/active FG.
use super::*;
use windows::{
    Win32::{
        Graphics::{Direct3D12::ID3D12Resource, Dxgi::IDXGISwapChain3},
        UI::WindowsAndMessaging::*,
    },
    core::{Interface, w},
};

struct Window(windows::Win32::Foundation::HWND);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) }.unwrap();
    }
}

unsafe fn assert_proxy<T: Interface>(runtime: &Runtime, proxy: &T) -> T {
    type GetNative = unsafe extern "C" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> i32;
    let get = unsafe { runtime._library.get::<GetNative>(b"slGetNativeInterface\0") }.unwrap();
    let mut native = std::ptr::null_mut();
    assert_eq!(unsafe { get(proxy.as_raw(), &mut native) }, 0);
    assert!(!native.is_null());
    assert_ne!(
        native,
        proxy.as_raw(),
        "creation returned a native interface"
    );
    // slGetNativeInterface always returns one owned reference.
    let owner = unsafe { windows::core::IUnknown::from_raw(native) };
    owner.cast::<T>().unwrap()
}

#[test]
#[ignore = "requires the official SDK, Windows desktop and an RTX DX12 FG device"]
fn official_sdk_dx12_creation_buffers_resize_and_recreate() {
    // Runtime-last caught a missing D3D12 DLL pin even though Runtime-first
    // passed. Reinitialize after each order to detect a retained SDK lease.
    for runtime_first in [false, true] {
        creation_and_teardown(runtime_first);
    }
    let path = std::env::var_os("HORIZON_STREAMLINE_SDK").expect("SDK path");
    drop(
        Runtime::load_with(
            Path::new(&path),
            "00a6bdb2-6386-4e37-8ff9-dcda4642628a",
            Api::Dx12,
            false,
            true,
            true,
        )
        .unwrap(),
    );
    assert!(!SESSION.load(Ordering::Acquire));
}

fn creation_and_teardown(runtime_first: bool) {
    let path = std::env::var_os("HORIZON_STREAMLINE_SDK").expect("SDK path");
    eprintln!("creation: load SDK");
    let mut runtime = Runtime::load_with(
        Path::new(&path),
        "00a6bdb2-6386-4e37-8ff9-dcda4642628a",
        Api::Dx12,
        false,
        true,
        true,
    )
    .unwrap();
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    eprintln!("creation: init instance");
    let instance = runtime.create_instance(&desc).unwrap();
    {
        let hal = unsafe { instance.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
        unsafe { assert_proxy(&runtime, hal.raw_factory4()) };
    }
    eprintln!("creation: enumerate adapters");
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!("creation: open device/queue");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    eprintln!("creation: attach features");
    runtime.attach_device(&device).unwrap();
    runtime.check_present_features(&adapter);
    assert!(
        runtime.framegen_loaded(),
        "{:?}",
        runtime.take_framegen_error()
    );
    {
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
        unsafe { assert_proxy(&runtime, hal.raw_queue()) };
    }
    let window = Window(
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Horizon DX12 creation test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                800,
                600,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap(),
    );
    for generation in 0..2 {
        let handle = wgpu::rwh::Win32WindowHandle::new(
            std::num::NonZeroIsize::new(window.0.0 as isize).unwrap(),
        );
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(wgpu::rwh::RawDisplayHandle::Windows(
                    wgpu::rwh::WindowsDisplayHandle::new(),
                )),
                raw_window_handle: wgpu::rwh::RawWindowHandle::Win32(handle),
            })
        }
        .unwrap();
        let mut config = surface.get_default_config(&adapter, 800, 600).unwrap();
        for width in [800, 960, 800] {
            eprintln!("creation: configure generation={generation} width={width}");
            runtime.suspend_framegen().unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            config.width = width;
            surface.configure(&device, &config);
            runtime.note_dx12_swapchain(&surface).unwrap();
            let chain = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }
                .unwrap()
                .swap_chain()
                .unwrap();
            let native: IDXGISwapChain3 = unsafe { assert_proxy(&runtime, &chain) };
            let index = unsafe { chain.GetCurrentBackBufferIndex() };
            let proxy_buffer: ID3D12Resource = unsafe { chain.GetBuffer(index) }.unwrap();
            let native_buffer: ID3D12Resource = unsafe { native.GetBuffer(index) }.unwrap();
            assert_ne!(
                proxy_buffer.as_raw(),
                native_buffer.as_raw(),
                "DLSS-G did not replace the back buffer at creation"
            );
            let output = match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(output) => output,
                other => panic!("acquire: {other:?}"),
            };
            {
                let hal = unsafe { output.texture.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
                assert_eq!(
                    unsafe { hal.raw_resource() }.as_raw(),
                    proxy_buffer.as_raw(),
                    "HAL cached a buffer from outside the creation proxy"
                );
            }
            println!(
                "generation={generation} width={width}: proxy/native buffers differ={}",
                proxy_buffer.as_raw() != native_buffer.as_raw()
            );
            // Clear and submit through the production queue before Present.
            let view = output.texture.create_view(&Default::default());
            let mut encoder = device.create_command_encoder(&Default::default());
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            }));
            queue.submit([encoder.finish()]);
            queue.present(output);
        }
        runtime.suspend_framegen().unwrap();
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        drop(surface);
    }
    eprintln!("teardown runtime_first={runtime_first}");
    if runtime_first {
        drop(runtime);
        drop(queue);
        drop(device);
        drop(adapter);
        drop(instance);
    } else {
        drop(queue);
        drop(device);
        drop(adapter);
        drop(instance);
        drop(runtime);
    }
    eprintln!("teardown complete");
    assert!(
        !SESSION.load(Ordering::Acquire),
        "SDK lease leaked after teardown"
    );
}
