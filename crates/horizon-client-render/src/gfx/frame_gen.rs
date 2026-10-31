//! Frame generation: FSR 3.1's frame interpolation will show a frame halfway between each
//! two real ones. So far this holds its optical flow; the interpolation and the setting
//! that runs them come next.

// Unused until the interpolation and the renderer use it.
#[allow(dead_code)]
mod optical_flow;

/// Apply a pending lifecycle reset before either SR or FG builds constants.
/// Paused frames and an unavailable presentation hook must not consume it or
/// continually reset ordinary TAA.
pub(super) fn prepare_frame_history(
    eligible: bool,
    pending: bool,
    presentation_ready: bool,
    post: &mut super::world_post::Post,
    motion: Option<&mut super::motion::Motion>,
) {
    if eligible && pending && presentation_ready {
        post.reset_history();
        if let Some(motion) = motion {
            motion.reset();
        }
    }
}

pub(super) fn prepare_presentation(
    streamline: &Option<super::dlss::Shared>,
    device: &wgpu::Device,
) -> Result<(), String> {
    prepare_change(
        || {
            #[cfg(target_os = "windows")]
            if let Some(shared) = streamline {
                let mut runtime = shared
                    .lock()
                    .map_err(|_| "Streamline lock poisoned".to_owned())?;
                runtime.suspend_framegen()?;
            }
            #[cfg(not(target_os = "windows"))]
            let _ = streamline;
            Ok(())
        },
        || {
            // The pinned options use eBlockPresentingClientQueue (0): waiting
            // on this same queue after Off drains prior frame and tag work.
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(5)),
                })
                .map(|_| ())
                .map_err(|e| format!("Surface synchronization failed: {e}"))
        },
    )
}

/// The mutation's caller must propagate failure before touching the surface.
/// Separate boundaries let regression tests inject SDK and queue failures.
pub(super) fn prepare_change(
    off: impl FnOnce() -> Result<(), String>,
    synchronize: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    off()?;
    synchronize()
}

/// All surface creation and reconfiguration paths finish here before callers
/// can acquire/present. Hook failure permits ordinary rendering; the runtime
/// records why presentation-dependent features are unavailable.
pub(super) fn configure_presentation(
    configure: impl FnOnce(),
    hook: impl FnOnce() -> Result<(), String>,
) {
    configure();
    if let Err(error) = hook() {
        log::warn!("Streamline presentation unavailable: {error}");
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::prepare_change;
    use std::cell::RefCell;

    #[test]
    fn new_surface_is_configured_and_hooked_before_first_present() {
        let calls = RefCell::new(Vec::new());
        prepare_change(
            || {
                calls.borrow_mut().push("Off");
                Ok(())
            },
            || {
                calls.borrow_mut().push("wait");
                Ok(())
            },
        )
        .unwrap();
        calls.borrow_mut().push("create new surface");
        super::configure_presentation(
            || calls.borrow_mut().push("configure new chain"),
            || {
                calls.borrow_mut().push("hook new chain");
                Ok(())
            },
        );
        calls.borrow_mut().push("Present");
        assert_eq!(
            *calls.borrow(),
            [
                "Off",
                "wait",
                "create new surface",
                "configure new chain",
                "hook new chain",
                "Present"
            ]
        );
    }

    #[test]
    fn failed_hook_keeps_native_render_and_reports_before_present() {
        let calls = RefCell::new(Vec::new());
        let available = std::cell::Cell::new(true);
        super::configure_presentation(
            || calls.borrow_mut().push("configure"),
            || {
                calls.borrow_mut().push("hook failed");
                available.set(false);
                Err("slUpgradeInterface failed (42)".into())
            },
        );
        assert!(!available.get());
        calls.borrow_mut().push("native Present");
        assert_eq!(
            *calls.borrow(),
            ["configure", "hook failed", "native Present"]
        );
    }

    #[test]
    fn first_eligible_frame_resets_history_after_multiple_menu_frames() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, _) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut post =
            super::super::world_post::Post::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        post.track_camera = true;
        // SR Off: menu frames still update the tracked temporal camera, just
        // like the bug report. Their reset goes true, false, false.
        for expected in [1.0, 0.0, 0.0] {
            super::prepare_frame_history(false, true, true, &mut post, None);
            post.jitter([1280, 720], false, glam::Mat4::IDENTITY, glam::DVec3::ZERO);
            assert_eq!(post.temporal_jitter().unwrap()[2], expected);
        }
        super::prepare_frame_history(true, true, true, &mut post, None);
        post.jitter([1280, 720], false, glam::Mat4::IDENTITY, glam::DVec3::ZERO);
        assert_eq!(post.temporal_jitter().unwrap()[2], 1.0);
        // A successful native On clears pending; ordinary frames resume.
        super::prepare_frame_history(true, false, true, &mut post, None);
        post.jitter([1280, 720], false, glam::Mat4::IDENTITY, glam::DVec3::ZERO);
        assert_eq!(post.temporal_jitter().unwrap()[2], 0.0);
    }

    #[test]
    fn hook_failure_preserves_fallback_taa_history_until_recovery() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, _) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut post =
            super::super::world_post::Post::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        assert!(post.supports_taa());
        post.taa = true;
        post.track_camera = true;
        // Production reset decision: FG is wanted/eligible and Session retains
        // its pending reset, but the failed hook blocks native FG recording.
        // Real Post camera history must keep advancing in TAA fallback.
        for (frame, expected) in [1.0, 0.0, 0.0, 0.0].into_iter().enumerate() {
            super::prepare_frame_history(true, true, false, &mut post, None);
            post.jitter(
                [1280, 720],
                true,
                glam::Mat4::IDENTITY,
                glam::DVec3::new(frame as f64, 0.0, 0.0),
            );
            assert_eq!(post.temporal_jitter().unwrap()[2], expected);
        }
        // A successful hook makes the still-pending reset applicable to the
        // first recovered FG frame, before constants are built.
        super::prepare_frame_history(true, true, true, &mut post, None);
        post.jitter([1280, 720], true, glam::Mat4::IDENTITY, glam::DVec3::ZERO);
        assert_eq!(post.temporal_jitter().unwrap()[2], 1.0);
        super::prepare_frame_history(true, false, true, &mut post, None);
        post.jitter([1280, 720], true, glam::Mat4::IDENTITY, glam::DVec3::ZERO);
        assert_eq!(post.temporal_jitter().unwrap()[2], 0.0);
    }

    #[test]
    fn checked_off_and_queue_drain_precede_mutation() {
        let calls = RefCell::new(Vec::new());
        let result = prepare_change(
            || {
                calls.borrow_mut().push("SDK Off");
                Ok(())
            },
            || {
                calls.borrow_mut().push("queue wait");
                Ok(())
            },
        )
        .map(|()| calls.borrow_mut().push("surface mutation"));
        assert!(result.is_ok());
        assert_eq!(
            *calls.borrow(),
            ["SDK Off", "queue wait", "surface mutation"]
        );
    }

    #[test]
    fn failed_off_or_drain_never_mutates_the_surface() {
        for fail_off in [true, false] {
            let calls = RefCell::new(Vec::new());
            let result = prepare_change(
                || {
                    calls.borrow_mut().push("SDK Off");
                    if fail_off {
                        Err("SDK error".into())
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("queue wait");
                    Err("wait failed".into())
                },
            )
            .map(|()| calls.borrow_mut().push("mutation"));
            assert!(result.is_err());
            assert_eq!(calls.borrow().len(), if fail_off { 1 } else { 2 });
        }
    }
}
