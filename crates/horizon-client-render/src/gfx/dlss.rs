//! Safe renderer-side control; all native handles remain in horizon-streamline.
use crate::graphics::{self, Dlss, DlssPreset, DlssReport};
use glam::Mat4;
#[cfg(target_os = "windows")]
pub(super) type Shared = std::sync::Arc<std::sync::Mutex<horizon_streamline::Runtime>>;
#[cfg(not(target_os = "windows"))]
pub(super) type Shared = ();

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

pub(super) fn load(
    dlss: bool,
    reflex: bool,
    framegen: bool,
    backend: wgpu::Backends,
) -> Option<Shared> {
    if (!dlss && !reflex && !framegen)
        || !matches!(backend, wgpu::Backends::VULKAN | wgpu::Backends::DX12)
    {
        return None;
    }
    #[cfg(target_os = "windows")]
    {
        let api = if backend == wgpu::Backends::DX12 {
            horizon_streamline::Api::Dx12
        } else {
            horizon_streamline::Api::Vulkan
        };
        let path = std::env::var_os("HORIZON_STREAMLINE_SDK")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.join("streamline")))
            });
        let result = path
            .ok_or("Streamline directory missing".to_owned())
            .and_then(|p| {
                horizon_streamline::Runtime::load_with(
                    &p,
                    "00a6bdb2-6386-4e37-8ff9-dcda4642628a",
                    api,
                    dlss,
                    reflex,
                    framegen,
                )
            });
        match result {
            Ok(runtime) => {
                if reflex && !runtime.reflex_requested() {
                    log::warn!("Reflex plugins were not found beside the Streamline SDK");
                }
                if framegen && !runtime.framegen_requested() {
                    log::warn!("Frame Generation plugins were not found beside the Streamline SDK");
                }
                if framegen && !runtime.reflex_requested() {
                    log::warn!("Reflex plugins were not found; Frame Generation needs them");
                }
                let shared = std::sync::Arc::new(std::sync::Mutex::new(runtime));
                horizon_streamline::install_reflex(&shared);
                Some(shared)
            }
            Err(e) => {
                if dlss {
                    graphics::report_dlss("TAA: SDK error");
                }
                log::warn!("Streamline unavailable: {e}");
                None
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if dlss {
            graphics::report_dlss("TAA: Windows");
        }
        None
    }
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
    pub(super) fn invalidate_history(&mut self) {
        self.reset = true;
    }

    pub fn new(shared: Option<Shared>, requested: bool, info: &wgpu::AdapterInfo) -> Self {
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
                requested
                    .then(graphics::dlss_status)
                    .filter(|s| s.starts_with("TAA:"))
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
        projection: Mat4,
        reproject: Mat4,
        jitter: [f32; 4],
        size: [u32; 2],
        far: f32,
        fov: f32,
        aspect: f32,
        view: Mat4,
        camera: glam::DVec3,
    ) -> Option<wgpu::TextureView> {
        #[cfg(target_os = "windows")]
        {
            let engine = self.engine.as_mut()?;
            let frame = frame_constants(
                projection,
                reproject,
                jitter,
                size,
                far,
                fov,
                aspect,
                view,
                camera,
                self.reset || jitter[2] != 0.0,
            );
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
            let _ = (
                color, depth, motion, projection, reproject, jitter, size, far, fov, aspect, view,
                camera,
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unsupported_platform_never_loads_a_streamline_session() {
        for backend in [wgpu::Backends::VULKAN, wgpu::Backends::METAL] {
            assert!(load(true, true, true, backend).is_none());
        }
        let state = State::new(
            None,
            true,
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
            let mut state = State::new(None, true, &info);
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
                false,
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
            false,
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
