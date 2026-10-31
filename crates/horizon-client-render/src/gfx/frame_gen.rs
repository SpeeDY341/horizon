//! Frame generation: FSR 3.1's frame interpolation will show a frame halfway between each
//! two real ones. So far this holds its optical flow; the interpolation and the setting
//! that runs them come next.

// Unused until the interpolation and the renderer use it.
#[allow(dead_code)]
mod optical_flow;

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

#[cfg(test)]
mod lifecycle_tests {
    use super::prepare_change;
    use std::cell::RefCell;

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
