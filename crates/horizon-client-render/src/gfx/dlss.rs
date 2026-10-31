//! Safe renderer-side control; all native handles remain in horizon-streamline.
use crate::graphics::{self, Dlss};
use glam::Mat4;
#[cfg(target_os = "windows")]
pub(super) type Shared = std::sync::Arc<std::sync::Mutex<horizon_streamline::Runtime>>;
#[cfg(not(target_os = "windows"))]
pub(super) type Shared = ();

pub(super) fn load(enabled: bool, backend: wgpu::Backends) -> Option<Shared> {
    if !enabled || backend != wgpu::Backends::VULKAN {
        return None;
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
        let result = path
            .ok_or("Streamline directory missing".to_owned())
            .and_then(|p| {
                horizon_streamline::Runtime::load(&p, "00a6bdb2-6386-4e37-8ff9-dcda4642628a")
            });
        match result {
            Ok(runtime) => Some(std::sync::Arc::new(std::sync::Mutex::new(runtime))),
            Err(e) => {
                graphics::report_dlss(format!("TAA: {e}"));
                log::warn!("DLSS unavailable: {e}");
                None
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        graphics::report_dlss("TAA: Streamline requires Windows");
        None
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
}
impl State {
    pub fn new(shared: Option<Shared>, requested: bool) -> Self {
        Self {
            requested: Dlss::Off,
            shared,
            #[cfg(target_os = "windows")]
            engine: None,
            failure: None,
            startup_failure: requested
                .then(graphics::dlss_status)
                .filter(|s| s.starts_with("TAA:")),
            configuration: None,
            reset: true,
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
    #[cfg(target_os = "windows")]
    fn fail(&mut self, e: String) {
        log::warn!("DLSS disabled, using TAA: {e}");
        graphics::report_dlss(format!("TAA: {e}"));
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
        if let Some(e) = &self.failure {
            graphics::report_dlss(format!("TAA: {e}"));
            return None;
        }
        if hdr {
            graphics::report_dlss("TAA: HDR enabled");
            self.reset = true;
            return None;
        }
        if !allowed {
            graphics::report_dlss("Paused: menu or shader pack");
            self.reset = true;
            return None;
        }
        #[cfg(target_os = "windows")]
        {
            let Some(shared) = &self.shared else {
                graphics::report_dlss(
                    self.startup_failure
                        .clone()
                        .unwrap_or_else(|| "TAA: restart required".into()),
                );
                return None;
            };
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
            }
            match self
                .engine
                .as_mut()
                .unwrap()
                .configure(self.requested.mode(), output)
            {
                Ok(size) => {
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
            let _ = (device, queue, output, &self.shared);
            graphics::report_dlss("TAA: Streamline requires Windows");
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
            let frame = horizon_streamline::FrameConstants {
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
                reset: self.reset || jitter[2] != 0.0,
            };
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
                    graphics::report_dlss(format!("Active: {}", self.requested.label()));
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
                color, depth, motion, projection, reproject, jitter, size, far, fov, aspect,
            );
            None
        }
    }
}
