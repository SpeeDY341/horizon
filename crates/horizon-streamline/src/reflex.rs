/*
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
/*
* Copyright (c) 2023 NVIDIA CORPORATION. All rights reserved
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
// Rust ABI translation of Streamline 2.14.16 include/sl_reflex.h, include/sl_pcl.h.
// Pinned source: NVIDIA-RTX/Streamline 625db07d4bca5abc67d781c42af7e7fb0761f1b6.
//! Reflex and PCL for one presented frame. The window thread owns the frame
//! token. The server thread may only set a simulation marker on a token the
//! window has already published, and only if the window has not marked it.
//! `frameLimitUs` stays 0: Reflex must not replace the game's frame cap.
use crate::{abi::Base, dlss_abi::GetFunction, runtime::Runtime};
use std::{
    ffi::{CStr, c_void},
    sync::{Arc, Mutex, Weak},
};

const INPUT_SAMPLE: u32 = 6;
const SIM_START: u32 = 0;
const SIM_END: u32 = 1;
const SUBMIT_START: u32 = 2;
const SUBMIT_END: u32 = 3;
const PRESENT_START: u32 = 4;
const PRESENT_END: u32 = 5;

/// ReflexState v2: base, two bools, padding, message id, then 64 reports.
const REPORT_AT: usize = 40;
const REPORT_STRIDE: usize = 152;
const REPORTS: usize = 64;
/// Offsets inside one ReflexReport, which itself starts with a 32-byte base.
const FRAME_ID: usize = 32;
const INPUT_TIME: usize = 40;
const SIM_TIME: usize = 48;
const PRESENT_DONE: usize = 88;
const GPU_DONE: usize = 136;

type NewToken = unsafe extern "C" fn(*mut *mut c_void, *const u32) -> i32;
type SetOptions = unsafe extern "C" fn(*const ReflexOptions) -> i32;
type GetState = unsafe extern "C" fn(*mut c_void) -> i32;
type Sleep = unsafe extern "C" fn(*const c_void) -> i32;
type Marker = unsafe extern "C" fn(u32, *const c_void) -> i32;

#[repr(C)]
struct ReflexOptions {
    base: Base,
    mode: u32,
    frame_limit_us: u32,
    use_markers_to_optimize: bool,
    _pad: u8,
    virtual_key: u16,
    id_thread: u32,
}

#[repr(C, align(16))]
struct StateBuf([u8; 16384]);

struct Api {
    new_token: NewToken,
    set_options: SetOptions,
    get_state: GetState,
    sleep: Sleep,
    marker: Marker,
}

pub struct Session {
    requested: bool,
    loaded: bool,
    api: Option<Api>,
    mode: u32,
    options_sent: bool,
    options_attempted: bool,
    /// Address of the token whose frame is on screen. Zero means none.
    current: usize,
    index: u32,
    pending: usize,
    /// 0 none, 1 window, 2 server, 3 ended.
    sim: u8,
    open: bool,
    submitting: bool,
    error: Option<String>,
    reported: bool,
    diagnostic: Option<String>,
    latency_us: u64,
    state: StateBuf,
}

impl Session {
    pub fn new(requested: bool) -> Self {
        Self {
            requested,
            loaded: false,
            api: None,
            mode: 0,
            options_sent: false,
            options_attempted: false,
            current: 0,
            index: 0,
            pending: 0,
            sim: 0,
            open: false,
            submitting: false,
            error: None,
            reported: false,
            diagnostic: None,
            latency_us: 0,
            state: StateBuf([0; 16384]),
        }
    }

    pub fn requested(&self) -> bool {
        self.requested
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn active(&self) -> bool {
        self.loaded
            && self.options_sent
            && self.mode > 0
            && self.state.0[32] != 0
            && self.diagnostic.is_none()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn token(&self) -> *mut c_void {
        self.current as *mut c_void
    }

    pub fn line(&self) -> String {
        if let Some(error) = &self.diagnostic {
            return format!("Reflex: unavailable ({error})");
        }
        if !self.loaded {
            return String::new();
        }
        if self.mode == 0 {
            return "Reflex: off".into();
        }
        if self.state.0[33] == 0 {
            return "Reflex: latency reports unavailable".into();
        }
        if self.latency_us == 0 {
            return "Reflex: waiting for latency reports".into();
        }
        format!("Reflex: {:.1} ms", self.latency_us as f64 / 1000.0)
    }

    pub fn fail(&mut self, error: String) {
        self.loaded = false;
        self.note_error(error);
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    pub fn set_mode(&mut self, mode: u32) {
        if self.mode != mode {
            self.mode = mode;
            self.options_sent = false;
            self.options_attempted = false;
            if self.loaded {
                self.diagnostic = None;
            }
            self.reported = false;
            self.latency_us = 0;
        }
        self.send_options();
    }

    /// Bind after the device exists. `slGetFeatureFunction` fails before that.
    pub fn bind(&mut self, library: &libloading::os::windows::Library) -> Result<(), String> {
        if !self.requested || self.api.is_some() {
            return Ok(());
        }
        // Safety: the library outlives the pointers, and the names are the
        // pinned public ABI. Feature functions are resolved only now.
        let api = unsafe {
            let get = *library
                .get::<GetFunction>(b"slGetFeatureFunction\0")
                .map_err(|e| e.to_string())?;
            let new_token = *library
                .get::<NewToken>(b"slGetNewFrameToken\0")
                .map_err(|e| e.to_string())?;
            let function = |feature: u32, name: &CStr| -> Result<*mut c_void, String> {
                let mut ptr = std::ptr::null_mut();
                let code = get(feature, name.as_ptr(), &mut ptr);
                if code != 0 || ptr.is_null() {
                    return Err(format!(
                        "Reflex function {} missing ({code})",
                        name.to_string_lossy()
                    ));
                }
                Ok(ptr)
            };
            Api {
                new_token,
                set_options: std::mem::transmute::<*mut c_void, SetOptions>(function(
                    3,
                    c"slReflexSetOptions",
                )?),
                get_state: std::mem::transmute::<*mut c_void, GetState>(function(
                    3,
                    c"slReflexGetState",
                )?),
                sleep: std::mem::transmute::<*mut c_void, Sleep>(function(3, c"slReflexSleep")?),
                marker: std::mem::transmute::<*mut c_void, Marker>(function(4, c"slPCLSetMarker")?),
            }
        };
        self.api = Some(api);
        self.query_state()?;
        if self.state.0[32] == 0 {
            return Err("Reflex low latency is unavailable on this device".into());
        }
        self.loaded = true;
        self.send_options();
        Ok(())
    }

    pub fn begin(&mut self, allow_sleep: bool) {
        if !self.loaded {
            return;
        }
        if self.open {
            self.abandon();
        }
        let token = if self.pending != 0 {
            let token = self.pending;
            self.pending = 0;
            token
        } else {
            let Ok(token) = self.allocate() else {
                return;
            };
            if allow_sleep {
                self.sleep(token);
            }
            token
        };
        self.current = token;
        self.open = true;
        self.sim = 0;
        self.send_options();
        // Marker 6 is the retired input sample. The checklist still requires it,
        // and the later values keep their numbers with the gap in place.
        self.mark(INPUT_SAMPLE);
        self.mark(SIM_START);
        self.sim = 1;
    }

    pub fn end_simulation(&mut self) {
        if self.sim == 1 {
            self.mark(SIM_END);
            self.sim = 3;
        }
    }

    pub fn submit(&mut self, start: bool) {
        if self.open && self.submitting != start {
            self.mark(if start { SUBMIT_START } else { SUBMIT_END });
            self.submitting = start;
        }
    }

    pub fn present(&mut self, start: bool) {
        if !self.open {
            return;
        }
        if start {
            self.mark(PRESENT_START);
            return;
        }
        self.mark(PRESENT_END);
        self.read_latency();
        self.index = self.index.wrapping_add(1);
        self.current = 0;
        self.sim = 0;
        self.open = false;
        if let Ok(token) = self.allocate() {
            self.sleep(token);
            self.pending = token;
        }
    }

    pub fn abandon_if_open(&mut self) {
        if self.open {
            self.abandon();
        }
    }

    /// A failed presentation hook cannot retire pending tokens. Do not reuse
    /// one after a later surface recovers.
    pub fn abandon_presentation(&mut self) {
        self.abandon();
        if self.pending != 0 {
            self.index = self.index.wrapping_add(1);
        }
        self.pending = 0;
    }

    /// Claim only a frame the window has opened and not yet given a simulation
    /// marker. The window sets that marker in the same step as the input
    /// marker, so this normally does not run.
    pub fn claim_server(&mut self) -> Option<usize> {
        if self.loaded && self.open && self.sim == 0 && self.current != 0 {
            self.mark(SIM_START);
            self.sim = 2;
            Some(self.current)
        } else {
            None
        }
    }

    pub fn finish_server(&mut self, token: usize) {
        if self.sim == 2 && self.current == token {
            self.mark(SIM_END);
            self.sim = 3;
        }
    }

    fn abandon(&mut self) {
        self.end_simulation();
        self.submit(false);
        if self.current != 0 {
            self.index = self.index.wrapping_add(1);
        }
        self.current = 0;
        self.sim = 0;
        self.open = false;
        self.submitting = false;
    }

    fn allocate(&mut self) -> Result<usize, String> {
        let Some(api) = &self.api else {
            return Err("Reflex is not loaded".into());
        };
        let mut token = std::ptr::null_mut();
        let index = self.index;
        // Safety: initialized SDK, writable token slot, index lives for the call.
        let code = unsafe { (api.new_token)(&mut token, &index) };
        if code != 0 || token.is_null() {
            let error = format!("Reflex frame token failed ({code})");
            self.note_error(error.clone());
            return Err(error);
        }
        Ok(token as usize)
    }

    fn sleep(&mut self, token: usize) {
        let Some(api) = &self.api else {
            return;
        };
        if token == 0 {
            return;
        }
        // Safety: token came from slGetNewFrameToken and has not been retired.
        // Sleep is thread-safe. The limit stays with the game: frameLimitUs is 0.
        let code = unsafe { (api.sleep)(token as *const c_void) };
        if code != 0 {
            self.note_error(format!("Reflex sleep failed ({code})"));
        }
    }

    fn mark(&mut self, marker: u32) {
        let Some(api) = &self.api else {
            return;
        };
        if self.current == 0 {
            return;
        }
        // Safety: the marker id is from the pinned PCL enum, and the token is
        // the one this frame is presenting. The call is thread-safe.
        let code = unsafe { (api.marker)(marker, self.current as *const c_void) };
        if code != 0 {
            self.note_error(format!("Reflex marker {marker} failed ({code})"));
        }
    }

    fn send_options(&mut self) {
        let Some(api) = &self.api else {
            return;
        };
        if !self.loaded || self.options_attempted {
            return;
        }
        self.options_attempted = true;
        let options = ReflexOptions {
            base: Base::new(
                0xf03af81a,
                0x6d0b,
                0x4902,
                [0xa6, 0x51, 0xc4, 0x96, 0x5e, 0x21, 0x54, 0x34],
                1,
            ),
            mode: self.mode,
            frame_limit_us: 0,
            use_markers_to_optimize: false,
            _pad: 0,
            virtual_key: 0,
            id_thread: 0,
        };
        // Safety: the options match the pinned v1 layout and outlive the call.
        let code = unsafe { (api.set_options)(&options) };
        if code != 0 {
            self.note_error(format!("Reflex options failed ({code})"));
            return;
        }
        self.options_sent = true;
    }

    fn read_latency(&mut self) {
        if let Err(error) = self.query_state() {
            self.latency_us = 0;
            self.note_error(error);
            return;
        }
        if self.state.0[32] == 0 {
            self.latency_us = 0;
            self.note_error("Reflex low latency is unavailable on this device".into());
            return;
        }
        if self.state.0[33] == 0 {
            self.latency_us = 0;
            return;
        }
        self.read_reports();
    }

    fn query_state(&mut self) -> Result<(), String> {
        let Some(api) = &self.api else {
            return Err("Reflex is not loaded".into());
        };
        let base = Base::new(
            0xf0bb5985,
            0xdaf9,
            0x4728,
            [0xb2, 0xfd, 0xae, 0x80, 0xa2, 0xbd, 0x79, 0x89],
            2,
        );
        // Safety: the buffer is 16 KiB and aligned. ReflexState v2 is smaller
        // than that. The header tells the SDK which version to fill.
        let code = unsafe {
            self.state.0.fill(0);
            std::ptr::write(self.state.0.as_mut_ptr() as *mut Base, base);
            (api.get_state)(self.state.0.as_mut_ptr() as *mut c_void)
        };
        if code != 0 {
            return Err(format!("Reflex state failed ({code})"));
        }
        Ok(())
    }

    fn read_reports(&mut self) {
        let mut best_id = 0u64;
        let mut best = 0u64;
        for i in 0..REPORTS {
            let at = REPORT_AT + i * REPORT_STRIDE;
            let id = read_u64(&self.state.0, at + FRAME_ID);
            let input = read_u64(&self.state.0, at + INPUT_TIME);
            let sim = read_u64(&self.state.0, at + SIM_TIME);
            let present = read_u64(&self.state.0, at + PRESENT_DONE);
            let gpu = read_u64(&self.state.0, at + GPU_DONE);
            let (start, end) = if input != 0 && gpu != 0 {
                (input, gpu)
            } else if sim != 0 && present != 0 {
                (sim, present)
            } else {
                continue;
            };
            if end > start && id >= best_id {
                best_id = id;
                // Streamline copies microsecond timestamps from its native
                // latency reports; these are not performance-counter ticks.
                best = end - start;
            }
        }
        self.latency_us = best;
    }

    fn note_error(&mut self, error: String) {
        self.diagnostic = Some(error.clone());
        if self.reported {
            return;
        }
        self.reported = true;
        self.error = Some(error);
    }
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let Some(end) = offset.checked_add(8) else {
        return 0;
    };
    let Some(slice) = bytes.get(offset..end) else {
        return 0;
    };
    u64::from_le_bytes(slice.try_into().unwrap_or([0; 8]))
}

static HOOK: Mutex<Weak<Mutex<Runtime>>> = Mutex::new(Weak::new());

/// Remember the runtime so the server thread can mark a tick. Skipped when
/// Reflex was not requested, so a DLSS-only frame does not take the lock.
pub fn install(shared: &Arc<Mutex<Runtime>>) {
    let requested = shared
        .lock()
        .ok()
        .is_some_and(|runtime| runtime.reflex_requested());
    if !requested {
        return;
    }
    if let Ok(mut hook) = HOOK.lock() {
        *hook = Arc::downgrade(shared);
    }
}

fn runtime() -> Option<Arc<Mutex<Runtime>>> {
    let hook = HOOK.lock().ok()?;
    hook.upgrade()
}

/// Brackets one server tick. The guard drops on every return, including an error.
pub struct ReflexServerTick {
    token: Option<usize>,
}

pub fn reflex_server_tick() -> ReflexServerTick {
    let token = runtime().and_then(|arc| {
        let mut runtime = arc.lock().ok()?;
        runtime.reflex_claim_server()
    });
    ReflexServerTick { token }
}

impl Drop for ReflexServerTick {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        let Some(arc) = runtime() else {
            return;
        };
        if let Ok(mut runtime) = arc.lock() {
            runtime.reflex_finish_server(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static MOCK: std::cell::RefCell<(Vec<u32>, i32, u8)> = const { std::cell::RefCell::new((Vec::new(), 0, 1)) };
    }
    unsafe extern "C" fn mock_options(options: *const ReflexOptions) -> i32 {
        MOCK.with_borrow_mut(|(calls, code, _)| {
            calls.push(unsafe { (*options).mode });
            *code
        })
    }
    unsafe extern "C" fn mock_state(state: *mut c_void) -> i32 {
        unsafe { *(state as *mut u8).add(32) = MOCK.with_borrow(|(_, _, available)| *available) };
        0
    }
    unsafe extern "C" fn mock_token(_: *mut *mut c_void, _: *const u32) -> i32 {
        1
    }
    unsafe extern "C" fn mock_sleep(_: *const c_void) -> i32 {
        0
    }
    unsafe extern "C" fn mock_marker(_: u32, _: *const c_void) -> i32 {
        0
    }

    fn report(session: &mut Session, slot: usize, id: u64, times: [u64; 4]) {
        let at = REPORT_AT + slot * REPORT_STRIDE;
        for (offset, value) in [FRAME_ID, INPUT_TIME, SIM_TIME, PRESENT_DONE, GPU_DONE]
            .into_iter()
            .zip([id, times[0], times[1], times[2], times[3]])
        {
            session.state.0[at + offset..at + offset + 8].copy_from_slice(&value.to_le_bytes());
        }
    }

    #[test]
    fn report_microseconds_are_preserved_in_the_overlay() {
        let mut session = Session::new(true);
        session.loaded = true;
        session.mode = 1;
        session.state.0[33] = 1;
        // SDK report timestamps are microseconds, including at large clock origins.
        for start in [1_000_000, u64::MAX - 20_000] {
            report(&mut session, 0, 1, [start, 1, 50_001, start + 20_000]);
            session.read_reports();
            assert_eq!(session.latency_us, 20_000);
            assert_eq!(session.line(), "Reflex: 20.0 ms");
        }
    }

    #[test]
    fn reports_select_the_newest_valid_frame_instead_of_the_last_slot() {
        let mut session = Session::new(true);
        report(&mut session, 0, 8, [100, 0, 0, 20_100]);
        report(&mut session, 1, 9, [100, 0, 0, 100]); // Equal timestamps.
        report(&mut session, 2, 10, [200, 0, 0, 100]); // Reversed timestamps.
        report(&mut session, 3, 11, [0; 4]); // Incomplete newer frame.
        report(&mut session, REPORTS - 1, 7, [100, 0, 0, 30_100]);
        session.read_reports();
        assert_eq!(session.latency_us, 20_000);
    }

    #[test]
    fn reports_fall_back_to_simulation_and_present_in_microseconds() {
        let mut session = Session::new(true);
        for (input, gpu) in [(0, 30_000), (1, 0), (0, 0)] {
            report(&mut session, 0, 1, [input, 100_000, 120_000, gpu]);
            session.read_reports();
            assert_eq!(session.latency_us, 20_000);
        }
    }

    #[test]
    fn missing_or_invalid_reports_clear_previous_latency() {
        let mut session = Session::new(true);
        for times in [[0; 4], [100, 0, 0, 100], [200, 0, 0, 100]] {
            session.latency_us = 20_000;
            report(&mut session, 0, 1, times);
            session.read_reports();
            assert_eq!(session.latency_us, 0);
        }
        report(&mut session, 0, 1, [1, 0, 0, u64::MAX]);
        session.read_reports();
        assert_eq!(session.latency_us, u64::MAX - 1);
    }

    #[test]
    fn failed_options_are_not_retried_each_frame_and_mode_change_recovers() {
        let mut session = Session::new(true);
        session.api = Some(Api {
            new_token: mock_token,
            set_options: mock_options,
            get_state: mock_state,
            sleep: mock_sleep,
            marker: mock_marker,
        });
        session.loaded = true;
        session.state.0[32] = 1;
        MOCK.with_borrow_mut(|state| *state = (Vec::new(), 42, 1));
        session.set_mode(1);
        assert!(!session.active());
        assert!(session.line().contains("options failed (42)"));
        assert!(session.take_error().is_some());
        for _ in 0..3 {
            session.set_mode(1);
        }
        assert!(session.take_error().is_none());
        MOCK.with_borrow_mut(|(_, code, _)| *code = 0);
        session.set_mode(2);
        assert!(session.active());
        MOCK.with_borrow(|(calls, _, _)| assert_eq!(calls, &[1, 2]));
        session.latency_us = 1234;
        session.read_latency();
        assert_eq!(session.latency_us, 0);
        assert_eq!(session.line(), "Reflex: latency reports unavailable");
        MOCK.with_borrow_mut(|(_, _, available)| *available = 0);
        session.read_latency();
        assert!(!session.active());
        assert!(session.line().contains("low latency is unavailable"));
    }

    #[test]
    fn unsupported_reflex_cannot_open_a_frame() {
        let mut session = Session::new(true);
        session.loaded = true;
        session.fail("unsupported adapter".into());
        assert!(!session.loaded());
        session.begin(false);
        assert!(!session.is_open());
        assert!(session.token().is_null());
        assert_eq!(session.take_error().as_deref(), Some("unsupported adapter"));
    }
    use std::mem::{offset_of, size_of};

    #[repr(C)]
    struct ReportLayout {
        base: Base,
        times: [u64; 14],
        gpu_active_us: u32,
        gpu_frame_us: u32,
    }
    #[repr(C)]
    struct StatePrefix {
        base: Base,
        low_latency_available: bool,
        latency_report_available: bool,
        _pad: [u8; 2],
        stats_window_message: u32,
    }

    #[test]
    fn options_and_reports_match_the_pinned_msvc_layout() {
        assert_eq!(size_of::<ReflexOptions>(), 48);
        assert_eq!(offset_of!(ReflexOptions, mode), 32);
        assert_eq!(offset_of!(ReflexOptions, frame_limit_us), 36);
        assert_eq!(offset_of!(ReflexOptions, virtual_key), 42);
        assert_eq!(offset_of!(ReflexOptions, id_thread), 44);
        assert_eq!(size_of::<ReportLayout>(), REPORT_STRIDE);
        assert_eq!(offset_of!(ReportLayout, times), 32);
        assert_eq!(size_of::<StatePrefix>(), REPORT_AT);
        assert_eq!(FRAME_ID + 13 * 8, GPU_DONE);
    }

    #[test]
    fn the_server_does_not_claim_a_frame_the_window_already_marked() {
        let mut session = Session::new(true);
        session.open = true;
        session.sim = 1;
        session.current = 4;
        assert!(session.claim_server().is_none());
        session.sim = 0;
        session.loaded = true;
        assert_eq!(session.claim_server(), Some(4));
        assert_eq!(session.sim, 2);
    }

    #[test]
    fn failed_presentation_discards_open_and_pending_tokens() {
        let mut session = Session::new(true);
        session.current = 4;
        session.open = true;
        session.submit(true);
        assert!(session.submitting);
        session.abandon_presentation();
        assert!(!session.submitting);
        assert_eq!(session.token(), std::ptr::null_mut());
        assert!(!session.is_open());
        assert_eq!(session.index, 1);
        session.pending = 5;
        session.abandon_presentation();
        assert_eq!(session.pending, 0);
        assert_eq!(session.index, 2);
    }
}
