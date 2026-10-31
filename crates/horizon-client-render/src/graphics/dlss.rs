//! User requests are kept distinct from what the running SDK confirms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DlssPreset {
    #[default]
    Auto,
    J,
    K,
    L,
    M,
    E,
    F,
}
impl DlssPreset {
    pub const ALL: [Self; 7] = [
        Self::Auto,
        Self::J,
        Self::K,
        Self::L,
        Self::M,
        Self::E,
        Self::F,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::E => "E",
            Self::F => "F",
        }
    }
    pub fn network(self) -> &'static str {
        match self {
            Self::Auto => "SDK default",
            Self::J | Self::K => "Transformer",
            Self::L | Self::M => "Transformer 2",
            Self::E | Self::F => "Legacy CNN request",
        }
    }
    pub fn value(self) -> u32 {
        match self {
            Self::Auto => 0,
            Self::J => 10,
            Self::K => 11,
            Self::L => 12,
            Self::M => 13,
            Self::E => 5,
            Self::F => 6,
        }
    }
    pub(super) fn parse(text: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(text))
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct DlssReport {
    pub versions: String,
    pub requested_preset: String,
    pub render_size: [u32; 2],
    pub output_size: [u32; 2],
    pub sdk_sizes: [[u32; 2]; 4],
    pub cpu_ms: [f32; 3],
    pub motion_cpu_ms: f32,
    pub gpu_ms: Option<[f32; 2]>,
    pub gpu_samples: u64,
    pub completed_frames: u64,
    pub reset_frames: u64,
}
static REPORT: std::sync::OnceLock<std::sync::Mutex<DlssReport>> = std::sync::OnceLock::new();
pub fn report_dlss_details(report: DlssReport) {
    *REPORT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = report;
}
pub fn dlss_details() -> DlssReport {
    REPORT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}
pub fn dlss_monitor_lines() -> Vec<String> {
    let r = dlss_details();
    let gpu = r.gpu_ms.map_or_else(
        || "GPU: awaiting samples / unavailable".into(),
        |[copy, dlss]| format!("GPU copy {copy:.3} / DLSS {dlss:.3} ms"),
    );
    let mut lines = vec![
        format!(
            "DLSS {} | preset request {}",
            super::dlss_status(),
            r.requested_preset
        ),
        r.versions,
        format!(
            "Render {}x{} -> {}x{}",
            r.render_size[0], r.render_size[1], r.output_size[0], r.output_size[1]
        ),
        format!(
            "CPU copy {:.3} / SDK {:.3} / total {:.3} ms",
            r.cpu_ms[0], r.cpu_ms[1], r.cpu_ms[2]
        ),
        gpu,
        format!(
            "Motion CPU {:.3} ms | frames {} / resets {}",
            r.motion_cpu_ms, r.completed_frames, r.reset_frames
        ),
        "Preset/network is requested; SDK or driver may substitute.".into(),
    ];
    if r.sdk_sizes[0][0] > 0 && r.sdk_sizes.windows(2).any(|s| s[0] == s[1]) {
        lines.push("WARNING: SDK returned equal sizes for different modes".into());
    }
    lines
}

static MONITOR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn set_dlss_monitor_enabled(on: bool) {
    MONITOR.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn dlss_monitor_enabled() -> bool {
    MONITOR.load(std::sync::atomic::Ordering::Relaxed)
}
