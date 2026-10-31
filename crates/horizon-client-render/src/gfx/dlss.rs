//! Safe renderer-side control; all native handles remain in horizon-streamline.
use crate::graphics::{self, Dlss, DlssPreset, DlssReport};
#[cfg(target_os = "windows")]
use glam::Mat4;
#[cfg(target_os = "windows")]
pub(super) type Shared = std::sync::Arc<std::sync::Mutex<horizon_streamline::Runtime>>;
#[cfg(not(target_os = "windows"))]
pub(super) type Shared = ();
#[cfg(target_os = "windows")]
pub(super) type Constants = horizon_streamline::FrameConstants;
#[cfg(not(target_os = "windows"))]
pub(super) type Constants = ();

pub(super) fn presentation_ready(shared: &Option<Shared>) -> bool {
    #[cfg(target_os = "windows")]
    if let Some(shared) = shared {
        return shared
            .lock()
            .is_ok_and(|runtime| runtime.presentation_ready().is_ok());
    }
    let _ = shared;
    true
}

/// Keep the runtime loaded even if a new native chain cannot be proxied: older
/// surfaces and native feature resources can still own SDK objects.
pub(super) fn hook_presentation(
    shared: &Option<Shared>,
    surface: &wgpu::Surface<'_>,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if let Some(shared) = shared {
        return shared
            .lock()
            .map_err(|_| "Streamline lock poisoned".to_owned())?
            .note_dx12_swapchain(surface);
    }
    let _ = (shared, surface);
    Ok(())
}

/// Immutable device-startup intent and failures. Kept beside the device, never
/// inferred from the current UI selection or the process-wide status overlay.
#[derive(Clone, Default)]
pub(super) struct Request {
    requested: bool,
    failure: Option<String>,
}
impl Request {
    pub(super) fn line(&self, enabled: bool, name: &str, runtime: Option<String>) -> String {
        if !enabled {
            return String::new();
        }
        if let Some(error) = &self.failure {
            return format!("{name}: unavailable ({error})");
        }
        if let Some(line) = runtime.filter(|line| !line.is_empty()) {
            return line;
        }
        if self.requested {
            format!("{name}: unavailable (startup plugin did not bind)")
        } else if !cfg!(target_os = "windows") {
            format!("{name}: unavailable (Windows required)")
        } else {
            format!("{name}: restart")
        }
    }
    fn fail(&mut self, error: &str) {
        if self.requested && self.failure.is_none() {
            self.failure = Some(error.into());
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct Startup {
    sr: Request,
    pub reflex: Request,
    pub framegen: Request,
}
impl Startup {
    pub(super) fn new(sr: bool, reflex: bool, framegen: bool) -> Self {
        let request = |requested| Request {
            requested,
            failure: None,
        };
        Self {
            sr: request(sr),
            reflex: request(reflex || framegen),
            framegen: request(framegen),
        }
    }
    pub(super) fn fail(&mut self, error: &str) {
        self.sr.fail(error);
        self.reflex.fail(error);
        self.framegen.fail(error);
    }
    #[cfg(any(target_os = "windows", test))]
    fn check_plugins(&mut self, directory: &std::path::Path) {
        let missing = |names: &[&'static str]| {
            names
                .iter()
                .copied()
                .filter(|name| !directory.join(name).is_file())
                .collect::<Vec<_>>()
        };
        let reflex = missing(&["sl.reflex.dll", "sl.pcl.dll"]);
        let fg = missing(&["sl.dlss_g.dll", "nvngx_dlssg.dll"]);
        let mut errors = Vec::new();
        if !fg.is_empty() {
            errors.push(format!("missing {}", fg.join(", ")));
        }
        if !reflex.is_empty() {
            let error = format!("missing {}", reflex.join(", "));
            self.reflex.fail(&error);
            errors.push(format!("Reflex required: {error}"));
        }
        if !errors.is_empty() {
            self.framegen.fail(&errors.join("; "));
        }
    }
}

pub(super) struct Loaded {
    pub shared: Option<Shared>,
    pub startup: Startup,
}

pub(super) fn load(dlss: bool, reflex: bool, framegen: bool, backend: wgpu::Backends) -> Loaded {
    let mut loaded = Loaded {
        shared: None,
        startup: Startup::new(dlss, reflex, framegen),
    };
    if !dlss && !reflex && !framegen {
        return loaded;
    }
    if !cfg!(target_os = "windows") {
        loaded.startup.fail("Windows required");
        return loaded;
    }
    if !matches!(backend, wgpu::Backends::VULKAN | wgpu::Backends::DX12) {
        loaded.startup.fail("Vulkan or DirectX 12 required");
        return loaded;
    }
    #[cfg(target_os = "windows")]
    {
        let path = std::env::var_os("HORIZON_STREAMLINE_SDK")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.join("streamline")))
            });
        load_directory(loaded, path.as_deref(), backend)
    }
    #[cfg(not(target_os = "windows"))]
    loaded
}

#[cfg(target_os = "windows")]
fn load_directory(
    mut loaded: Loaded,
    path: Option<&std::path::Path>,
    backend: wgpu::Backends,
) -> Loaded {
    let result = path
        .ok_or("Streamline directory missing".to_owned())
        .and_then(|path| {
            // Check only real files; the native loader still enforces SDK signatures.
            // Avoid replacing an SDK/path failure with a misleading plugin failure.
            if path.is_absolute() && path.join("sl.interposer.dll").is_file() {
                loaded.startup.check_plugins(path);
            }
            horizon_streamline::Runtime::load_with(
                path,
                "00a6bdb2-6386-4e37-8ff9-dcda4642628a",
                if backend == wgpu::Backends::DX12 {
                    horizon_streamline::Api::Dx12
                } else {
                    horizon_streamline::Api::Vulkan
                },
                loaded.startup.sr.requested,
                loaded.startup.reflex.requested,
                loaded.startup.framegen.requested,
            )
            .map_err(|error| format!("SDK {}: {error}", path.display()))
        });
    match result {
        Ok(runtime) => {
            let shared = std::sync::Arc::new(std::sync::Mutex::new(runtime));
            horizon_streamline::install_reflex(&shared);
            loaded.shared = Some(shared);
        }
        Err(error) => {
            log::warn!("Streamline unavailable: {error}");
            loaded.startup.fail(&error);
        }
    }
    loaded
}

/// Matrices without jitter, plus the jitter in pixels. Super Resolution and
/// Frame Generation share this so the two cannot drift.
#[cfg(target_os = "windows")]
pub(super) fn frame_constants(
    projection: Mat4,
    reproject: Mat4,
    jitter: [f32; 4],
    size: [u32; 2],
    far: f32,
    fov: f32,
    aspect: f32,
    view: Mat4,
    camera: glam::DVec3,
    reset: bool,
) -> horizon_streamline::FrameConstants {
    horizon_streamline::FrameConstants {
        projection: projection.to_cols_array_2d(),
        inverse_projection: projection.inverse().to_cols_array_2d(),
        reprojection: reproject.to_cols_array_2d(),
        inverse_reprojection: reproject.inverse().to_cols_array_2d(),
        jitter_pixels: [
            jitter[0] * size[0] as f32 * 0.5,
            -jitter[1] * size[1] as f32 * 0.5,
        ],
        camera: [
            camera.as_vec3().to_array(),
            view.inverse().transform_vector3(glam::Vec3::Y).to_array(),
            view.inverse().transform_vector3(glam::Vec3::X).to_array(),
            view.inverse().transform_vector3(-glam::Vec3::Z).to_array(),
        ],
        near: 0.05,
        far,
        fov,
        aspect,
        reset,
    }
}

pub(super) struct State {
    pub requested: Dlss,
    shared: Option<Shared>,
    #[cfg(target_os = "windows")]
    engine: Option<horizon_streamline::SuperResolution>,
    failure: Option<String>,
    startup_failure: Option<String>,
    configuration: Option<(Dlss, [u32; 2])>,
    reset: bool,
    preset: DlssPreset,
    pub monitoring: bool,
    pub motion_cpu_ms: f32,
    report: DlssReport,
    report_at: std::time::Instant,
    sums: [f32; 4],
    samples: u32,
}
impl State {
    #[cfg(target_os = "windows")]
    pub(super) fn reset_pending(&self) -> bool {
        self.reset && self.failure.is_none() && self.configuration.is_some()
    }
    pub(super) fn invalidate_history(&mut self) {
        self.reset = true;
    }

    pub fn new(shared: Option<Shared>, startup: &Startup, info: &wgpu::AdapterInfo) -> Self {
        Self {
            requested: Dlss::Off,
            shared,
            #[cfg(target_os = "windows")]
            engine: None,
            failure: None,
            startup_failure: if !cfg!(target_os = "windows") {
                Some("TAA: Windows".into())
            } else if !matches!(info.backend, wgpu::Backend::Vulkan | wgpu::Backend::Dx12)
                || info.vendor != 0x10de
                || !info.name.to_ascii_uppercase().contains("RTX")
            {
                Some("TAA: RTX".into())
            } else {
                startup
                    .sr
                    .failure
                    .as_ref()
                    .map(|error| format!("TAA: {error}"))
            },
            configuration: None,
            reset: true,
            preset: DlssPreset::Auto,
            monitoring: false,
            motion_cpu_ms: 0.0,
            report: DlssReport::default(),
            report_at: std::time::Instant::now(),
            sums: [0.0; 4],
            samples: 0,
        }
    }
    pub fn set(&mut self, requested: Dlss) {
        if self.requested != requested {
            self.failure = None;
            self.configuration = None;
            self.reset = true;
        }
        self.requested = requested;
    }
    pub fn set_options(&mut self, preset: DlssPreset, monitoring: bool) {
        if self.preset != preset {
            self.failure = None;
            self.configuration = None;
            self.reset = true;
            self.sums = [0.0; 4];
            self.samples = 0;
        }
        self.preset = preset;
        self.monitoring = monitoring;
        graphics::set_dlss_monitor_enabled(monitoring);
    }
    #[cfg(target_os = "windows")]
    fn record(&mut self) {
        let perf = self.engine.as_ref().unwrap().performance();
        let current = [
            perf.cpu_copy_ms,
            perf.cpu_evaluate_ms,
            perf.cpu_total_ms,
            self.motion_cpu_ms,
        ];
        for (sum, value) in self.sums.iter_mut().zip(current) {
            *sum += value;
        }
        self.samples += 1;
        if self.report_at.elapsed().as_millis() < 500 {
            return;
        }
        self.report.cpu_ms = std::array::from_fn(|i| self.sums[i] / self.samples as f32);
        self.report.motion_cpu_ms = self.sums[3] / self.samples as f32;
        self.report.gpu_ms = perf.gpu_ms;
        self.report.gpu_samples = perf.gpu_samples;
        self.report.completed_frames = perf.completed_frames;
        self.report.reset_frames = perf.reset_frames;
        graphics::report_dlss_details(self.report.clone());
        if self.monitoring
            && let Ok(json) = serde_json::to_string(&self.report)
        {
            log::info!("DLSS monitor: {json}");
        }
        self.report_at = std::time::Instant::now();
        self.sums = [0.0; 4];
        self.samples = 0;
    }
    #[cfg(target_os = "windows")]
    fn fail(&mut self, e: String) {
        log::warn!("DLSS disabled, using TAA: {e}");
        graphics::report_dlss("TAA: DLSS error");
        self.failure = Some(e);
        self.reset = true;
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        output: [u32; 2],
        allowed: bool,
        hdr: bool,
    ) -> Option<[u32; 2]> {
        if self.requested == Dlss::Off {
            graphics::report_dlss("Off");
            self.reset = true;
            return None;
        }
        if self.failure.is_some() {
            graphics::report_dlss("TAA: DLSS error");
            return None;
        }
        if hdr {
            graphics::report_dlss("TAA: HDR");
            self.reset = true;
            return None;
        }
        if !allowed {
            graphics::report_dlss("Suspended");
            self.reset = true;
            return None;
        }
        #[cfg(target_os = "windows")]
        {
            let Some(shared) = &self.shared else {
                graphics::report_dlss(
                    self.startup_failure
                        .clone()
                        .unwrap_or_else(|| "TAA: restart".into()),
                );
                return None;
            };
            // A hook failure is recoverable on the next configure. Do not make
            // it a sticky engine failure, or keep allocating unretired tokens.
            if let Err(error) = shared
                .lock()
                .map_err(|_| "Streamline lock poisoned".to_owned())
                .and_then(|runtime| runtime.presentation_ready())
            {
                graphics::report_dlss(format!("TAA: {error}"));
                self.reset = true;
                return None;
            }
            if self.report.versions.is_empty() {
                self.report.versions = shared
                    .lock()
                    .ok()
                    .and_then(|r| r.dlss_versions().ok())
                    .unwrap_or_else(|| "SDK version unavailable".into());
            }
            if self.engine.is_none() {
                match horizon_streamline::SuperResolution::new(shared.clone(), device, queue) {
                    Ok(e) => self.engine = Some(e),
                    Err(e) => {
                        self.fail(e);
                        return None;
                    }
                }
            }
            if self.configuration != Some((self.requested, output)) {
                self.reset = true;
                self.report.sdk_sizes = self
                    .engine
                    .as_ref()
                    .unwrap()
                    .recommended_sizes(output, self.preset.value())
                    .map(|table| std::array::from_fn(|i| table[i][0]))
                    .unwrap_or([[0; 2]; 4]);
            }
            self.engine
                .as_mut()
                .unwrap()
                .set_monitoring(self.monitoring);
            match self.engine.as_mut().unwrap().configure_with_preset(
                self.requested.mode(),
                output,
                self.preset.value(),
            ) {
                Ok(size) => {
                    if self.configuration != Some((self.requested, output)) {
                        log::info!(
                            "DLSS {} preset request {}: {}x{} -> {}x{}; SDK sizes {:?}",
                            self.requested.label(),
                            self.preset.name(),
                            size[0],
                            size[1],
                            output[0],
                            output[1],
                            self.report.sdk_sizes
                        );
                    }
                    self.report.requested_preset = self.preset.name().into();
                    self.report.render_size = size;
                    self.report.output_size = output;
                    graphics::report_dlss_details(self.report.clone());
                    self.configuration = Some((self.requested, output));
                    Some(size)
                }
                Err(e) => {
                    self.fail(e);
                    None
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (device, queue, output, &self.shared, &self.startup_failure);
            graphics::report_dlss("TAA: Windows");
            None
        }
    }
    pub fn resolve(
        &mut self,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        motion: &wgpu::TextureView,
        frame: Constants,
        size: [u32; 2],
    ) -> Option<wgpu::TextureView> {
        #[cfg(target_os = "windows")]
        {
            let engine = self.engine.as_mut()?;
            let result = if std::env::var_os("HORIZON_DLSS_FORCE_ERROR").is_some() {
                Err("Forced diagnostic failure".into())
            } else {
                engine.evaluate(color, depth, motion, frame)
            };
            match result {
                Ok(view) => {
                    if self.reset {
                        log::info!("DLSS Super Resolution active: {}", self.requested.label());
                    }
                    self.reset = false;
                    graphics::report_dlss(format!(
                        "Active {}x{} > {}x{}",
                        size[0], size[1], self.report.output_size[0], self.report.output_size[1]
                    ));
                    self.report.render_size = size;
                    self.record();
                    Some(view)
                }
                Err(e) => {
                    self.fail(e);
                    None
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (color, depth, motion, frame, size);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_diagnostics_preserve_requests_across_off_and_resource_reload() {
        let mut startup = Startup::new(true, true, true);
        startup.fail("device startup: adapter unsupported (code 7)");
        let reloaded = startup.clone(); // Renderer::new reuses the same device and metadata.
        for request in [&reloaded.reflex, &reloaded.framegen] {
            assert_eq!(request.line(false, "Feature", None), "");
            let line = request.line(true, "Feature", None);
            assert_eq!(
                line,
                "Feature: unavailable (device startup: adapter unsupported (code 7))"
            );
            assert_eq!(request.line(false, "Feature", Some(line.clone())), "");
            assert_eq!(request.line(true, "Feature", None), line);
            assert_eq!(
                request.line(true, "Feature", Some("Feature: off".into())),
                line
            );
        }
        let bound = Startup::new(false, true, true);
        for request in [&bound.reflex, &bound.framegen] {
            for reason in [
                "bind function missing (code 7)",
                "adapter unsupported (code 9)",
            ] {
                let line = format!("Feature: unavailable ({reason})");
                assert_eq!(request.line(true, "Feature", Some(line.clone())), line);
            }
        }
        let new_device = Startup::default();
        let expected = if cfg!(target_os = "windows") {
            "Reflex: restart"
        } else {
            "Reflex: unavailable (Windows required)"
        };
        assert_eq!(new_device.reflex.line(true, "Reflex", None), expected);
        assert!(new_device.sr.failure.is_none());
        assert!(
            startup
                .sr
                .failure
                .as_ref()
                .unwrap()
                .contains("adapter unsupported")
        );
    }

    #[test]
    fn startup_unsupported_api_is_unavailable_and_off_is_hidden() {
        let loaded = load(true, true, true, wgpu::Backends::GL);
        assert!(loaded.shared.is_none());
        for request in [&loaded.startup.reflex, &loaded.startup.framegen] {
            let line = request.line(true, "Feature", None);
            assert!(
                line.contains(if cfg!(target_os = "windows") {
                    "Vulkan or DirectX 12 required"
                } else {
                    "Windows required"
                }),
                "{line}"
            );
            assert!(!line.contains("restart"));
            assert!(request.line(false, "Feature", None).is_empty());
        }
    }

    #[test]
    fn absent_optional_plugins_keep_original_request_and_exact_names() {
        let scratch = std::env::temp_dir().join(format!(
            "horizon-f5-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&scratch).unwrap();
        let mut startup = Startup::new(true, true, true);
        startup.check_plugins(&scratch); // No DLLs, real or fake, are loaded.
        startup.fail("Streamline has no requested plugins");
        let reflex = startup.reflex.line(true, "Reflex", None);
        let fg = startup.framegen.line(true, "Frame Gen", None);
        for name in ["sl.reflex.dll", "sl.pcl.dll"] {
            assert!(reflex.contains(name), "{reflex}");
        }
        for name in ["sl.dlss_g.dll", "nvngx_dlssg.dll", "Reflex required"] {
            assert!(fg.contains(name), "{fg}");
        }
        assert!(startup.reflex.requested && startup.framegen.requested);
        std::fs::remove_dir(scratch).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn missing_sdk_and_wrong_path_report_load_error_instead_of_restart() {
        let scratch = std::env::temp_dir().join(format!(
            "horizon-f5-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&scratch).unwrap();
        for path in [
            scratch.clone(),
            scratch.join("missing"),
            std::path::PathBuf::from("relative-sdk"),
        ] {
            let loaded = load_directory(
                Loaded {
                    shared: None,
                    startup: Startup::new(true, true, true),
                },
                Some(&path),
                wgpu::Backends::DX12,
            );
            assert!(loaded.shared.is_none());
            for request in [&loaded.startup.reflex, &loaded.startup.framegen] {
                let line = request.line(true, "Feature", None);
                assert!(
                    line.contains("unavailable") && line.contains("SDK"),
                    "{line}"
                );
                assert!(!line.contains("restart"), "{line}");
            }
            let state = State::new(
                None,
                &loaded.startup,
                &adapter(wgpu::Backend::Dx12, 0x10de, "RTX 5090"),
            );
            assert!(state.startup_failure.as_ref().unwrap().contains("SDK"));
        }
        std::fs::remove_dir(scratch).unwrap();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unsupported_platform_never_loads_a_streamline_session() {
        for backend in [wgpu::Backends::VULKAN, wgpu::Backends::METAL] {
            assert!(load(true, true, true, backend).shared.is_none());
        }
        let state = State::new(
            None,
            &Startup::new(true, false, false),
            &adapter(wgpu::Backend::Vulkan, 0x10de, "RTX 5090"),
        );
        assert_eq!(state.startup_failure.as_deref(), Some("TAA: Windows"));
    }

    fn adapter(backend: wgpu::Backend, vendor: u32, name: &str) -> wgpu::AdapterInfo {
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, backend);
        info.name = name.into();
        info.vendor = vendor;
        info
    }
    #[test]
    fn unsupported_adapters_keep_an_accurate_fallback_reason() {
        for info in [
            adapter(wgpu::Backend::Dx12, 0x1002, "Radeon"),
            adapter(wgpu::Backend::Vulkan, 0x1002, "Radeon"),
            adapter(wgpu::Backend::Gl, 0x10de, "RTX 5090"),
        ] {
            let mut state = State::new(None, &Startup::new(true, false, false), &info);
            state.set(Dlss::Quality);
            assert!(state.startup_failure.as_ref().unwrap().starts_with("TAA:"));
            assert!(state.configuration.is_none());
        }
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn directx12_and_vulkan_rtx_can_host_dlss() {
        for backend in [wgpu::Backend::Dx12, wgpu::Backend::Vulkan] {
            let state = State::new(
                None,
                &Startup::default(),
                &adapter(backend, 0x10de, "NVIDIA GeForce RTX 5090"),
            );
            assert!(state.startup_failure.is_none(), "{backend:?}");
        }
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn an_error_stays_on_taa_until_the_user_changes_the_selection() {
        let mut state = State::new(
            None,
            &Startup::default(),
            &adapter(wgpu::Backend::Vulkan, 0x10de, "RTX 5090"),
        );
        state.set(Dlss::Quality);
        state.fail("diagnostic failure".into());
        state.set(Dlss::Quality);
        assert!(state.failure.is_some());
        state.set(Dlss::Off);
        state.set(Dlss::Quality);
        assert!(state.failure.is_none());
        assert!(state.reset);
    }
}
