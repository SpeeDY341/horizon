//! Retained surface requests. Failed preparation never consumes an intent.
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) enum Change {
    Resize(u32, u32),
    PresentMode(wgpu::PresentMode),
    Hdr(bool, [f32; 4], PathBuf),
    Fullscreen(Option<winit::window::Fullscreen>),
}

#[derive(Default)]
pub(super) struct Pending {
    changes: VecDeque<Change>,
    failures: u8,
    retry_at: Option<Instant>,
    terminal: Option<String>,
}

impl Pending {
    pub fn push(&mut self, change: Change) {
        // A resize drag or repeated settings application retains only the
        // latest request of each kind, at its original queue position. Moving
        // a failed request behind a successful no-op would reset its retries.
        if let Some(old) = self
            .changes
            .iter_mut()
            .find(|old| std::mem::discriminant(*old) == std::mem::discriminant(&change))
        {
            *old = change;
            return;
        }
        self.changes.push_back(change);
    }

    /// True means rendering may continue. Attempts are bounded even when a
    /// caller queues more changes, so persistent SDK failure cannot spin.
    pub fn poll(
        &mut self,
        now: Instant,
        mut apply: impl FnMut(&Change) -> Result<(), String>,
    ) -> Result<bool, String> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return Ok(false);
        }
        while let Some(change) = self.changes.front() {
            if let Err(error) = apply(change) {
                self.failures += 1;
                if self.failures == 1 {
                    log::warn!("Surface change deferred; retrying automatically: {error}");
                }
                if self.failures >= 3 {
                    let error = format!("Surface change failed after 3 attempts: {error}");
                    self.terminal = Some(error.clone());
                    return Err(error);
                }
                self.retry_at = Some(now + Duration::from_millis(250 * u64::from(self.failures)));
                return Ok(false);
            }
            self.changes.pop_front();
            self.failures = 0;
            self.retry_at = None;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_retries_off_before_mutation_and_resumes_without_another_event() {
        let now = Instant::now();
        let mut pending = Pending::default();
        pending.push(Change::Resize(1600, 900));
        let mut size = [800, 600];
        let mut calls = Vec::new();
        assert!(
            !pending
                .poll(now, |_| {
                    calls.push("Off error");
                    Err("SDK".into())
                })
                .unwrap()
        );
        assert_eq!(size, [800, 600]);
        assert!(
            !pending
                .poll(now + Duration::from_millis(249), |_| panic!("backoff"))
                .unwrap()
        );
        assert!(
            pending
                .poll(now + Duration::from_millis(250), |change| {
                    super::super::frame_gen::prepare_change(
                        || {
                            calls.push("Off ok");
                            Ok(())
                        },
                        || Ok(()),
                    )?;
                    if let Change::Resize(w, h) = change {
                        size = [*w, *h];
                    }
                    calls.push("resize");
                    Ok(())
                })
                .unwrap()
        );
        assert_eq!(size, [1600, 900]);
        assert_eq!(calls, ["Off error", "Off ok", "resize"]);
        assert!(
            pending
                .poll(now + Duration::from_secs(1), |_| panic!("already resumed"))
                .unwrap()
        );
    }

    #[test]
    fn repeated_failure_is_bounded_and_new_intents_do_not_bypass_backoff() {
        let now = Instant::now();
        let mut pending = Pending::default();
        pending.push(Change::Resize(1, 1));
        let mut attempts = 0;
        for (ms, expected) in [(0, 1), (1, 1), (250, 2), (251, 2), (750, 3), (1000, 3)] {
            pending.push(Change::Resize(2, 2));
            let result = pending.poll(now + Duration::from_millis(ms), |_| {
                attempts += 1;
                Err("Off failed".into())
            });
            assert_eq!(attempts, expected);
            assert_eq!(result.is_err(), ms >= 750);
            assert_eq!(pending.changes.len(), 1);
        }
    }

    #[test]
    fn resize_drag_cannot_move_failure_behind_noop_settings() {
        let now = Instant::now();
        let mut pending = Pending::default();
        let mut attempts = 0;
        let mut noop_calls = 0;
        // Loading's Resized handler follows resize with apply_menu_settings.
        // Replacing Resize must leave it ahead of the no-op PresentMode.
        for (ms, expected) in [(0, 1), (250, 2), (750, 3), (1000, 3)] {
            pending.push(Change::Resize(1600 + ms as u32, 900));
            pending.push(Change::PresentMode(wgpu::PresentMode::Fifo));
            let result = pending.poll(now + Duration::from_millis(ms), |change| match change {
                Change::Resize(..) => {
                    attempts += 1;
                    Err("Off failed".into())
                }
                Change::PresentMode(_) => {
                    noop_calls += 1;
                    Ok(())
                }
                _ => unreachable!(),
            });
            assert_eq!(attempts, expected);
            assert_eq!(noop_calls, 0);
            assert_eq!(result.is_err(), ms >= 750);
            assert_eq!(pending.failures, expected);
        }
    }

    #[test]
    fn settings_and_fullscreen_requests_are_retained_and_coalesced() {
        let now = Instant::now();
        let mut pending = Pending::default();
        pending.push(Change::PresentMode(wgpu::PresentMode::Fifo));
        pending.push(Change::Hdr(false, [0.0; 4], PathBuf::new()));
        pending.push(Change::Fullscreen(None));
        assert!(!pending.poll(now, |_| Err("Off error".into())).unwrap());
        pending.push(Change::PresentMode(wgpu::PresentMode::Mailbox));
        pending.push(Change::Hdr(true, [1.0; 4], PathBuf::from("marker")));
        pending.push(Change::Fullscreen(Some(
            winit::window::Fullscreen::Borderless(None),
        )));
        let mut applied = Vec::new();
        assert!(
            pending
                .poll(now + Duration::from_millis(250), |change| {
                    match change {
                        Change::PresentMode(mode) => {
                            assert_eq!(*mode, wgpu::PresentMode::Mailbox);
                            applied.push("present");
                        }
                        Change::Hdr(on, values, marker) => {
                            assert!(*on);
                            assert_eq!(*values, [1.0; 4]);
                            assert_eq!(marker, &PathBuf::from("marker"));
                            applied.push("HDR");
                        }
                        Change::Fullscreen(request) => {
                            assert!(request.is_some());
                            applied.push("fullscreen");
                        }
                        Change::Resize(..) => panic!("unexpected resize"),
                    }
                    Ok(())
                })
                .unwrap()
        );
        assert_eq!(applied, ["present", "HDR", "fullscreen"]);
    }
}
