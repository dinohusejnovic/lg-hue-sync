//! DRM-safe capture backend for webOS 24/25 via the undocumented
//! VtHistogram API hidden in `libvtcapture.so`.
//!
//! This backend never touches decoded video: it issues private V4L2 ioctls on
//! `/dev/video60` that make the video hardware return 16 spatial zones of
//! aggregate R/G/B histograms (256 bins each, 16-bit counts). Aggregate
//! statistics only — no frame buffers are read, no pixels leave the secure
//! video path. The histograms are converted to one average color per zone and
//! painted into a synthetic BGRA frame so the existing `ZoneSampler` pipeline
//! (smoothing, saturation weighting, letterbox detection, scene cuts) works
//! unchanged.
//!
//! Layout + calibration notes (measured on a webOS 25 OLED C5):
//! - Zone response equals the overlap fraction between the zone rectangle and
//!   the lit region, and each zone has a fixed per-read sample count
//!   (zone0 = 7200, zone4/8 = 7200 = halves, rest 704..2304).
//! - Fitted zone rectangles are stored in [`ZONE_RECTS`] as normalized
//!   fractional screen coordinates (x0,x1,y0,y1).
//! - Measured white response is 231.3/255 ≈ 0.907 linearly on every channel;
//!   primaries are gamut-compressed by the TV, so the calibration applies a
//!   white-point normalization only (neutral colors stay neutral), letting
//!   `saturation_boost` recover the rest.

use std::fs::OpenOptions;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use tracing::info;

use super::{CaptureError, CapturedFrame, ScreenCapture};

const DEVICE_PATH: &str = "/dev/video60";

/// Monotonic counters for live timing diagnostics (`/api/status` → `histogram_counters`).
/// Diff two readings to get rates; they cost one relaxed atomic add per frame.
pub static FRAME_REQUESTS: AtomicU64 = AtomicU64::new(0);
pub static READS: AtomicU64 = AtomicU64::new(0);
pub static CHANGED_READS: AtomicU64 = AtomicU64::new(0);
pub static FAILED_READS: AtomicU64 = AtomicU64::new(0);
pub static READ_NS_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static READ_NS_MAX: AtomicU64 = AtomicU64::new(0);

/// One histogram read for `/api/histogram/trace`: microseconds since the
/// backend started, mean luma of the 16 zones, and whether any raw payload
/// byte changed since the previous read (the hardware's true update cadence).
#[derive(Clone, Copy, serde::Serialize, utoipa::ToSchema)]
pub struct TraceEntry {
    pub t_us: u64,
    pub luma: u8,
    pub raw_changed: bool,
    /// Percent of green-plane samples in the darkest / brightest eighth of bins,
    /// over the two full-height halves (zones 4 and 8). Both high at once on a
    /// full-screen strobe means one read spans several video frames.
    pub dark_pct: u8,
    pub bright_pct: u8,
}

const TRACE_LEN: usize = 300;
pub static TRACE: std::sync::Mutex<std::collections::VecDeque<TraceEntry>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
static RAW_PAYLOAD_HASH: AtomicU64 = AtomicU64::new(0);
/// Reads whose raw payload differed from the previous read: the hardware's own
/// refresh count. Strobe boost uses it to tell real video from a phase-locked strobe.
pub static RAW_REFRESHES: AtomicU64 = AtomicU64::new(0);

fn trace_read(payload: &[u8], zone_colors: &[[u8; 4]; ZONES], epoch: Instant) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    payload.hash(&mut hasher);
    let hash = hasher.finish();
    let raw_changed = RAW_PAYLOAD_HASH.swap(hash, Ordering::Relaxed) != hash;
    if raw_changed {
        RAW_REFRESHES.fetch_add(1, Ordering::Relaxed);
    }
    let luma = (zone_colors
        .iter()
        .map(|c| (c[0] as u32 + c[1] as u32 + c[2] as u32) / 3)
        .sum::<u32>()
        / ZONES as u32) as u8;
    let (mut dark, mut bright, mut total) = (0u64, 0u64, 0u64);
    for zone in [4usize, 8] {
        let base = zone * RECORD_SIZE + 4 + BINS * 2; // green plane
        if let Some(plane) = payload.get(base..base + BINS * 2) {
            for (bin, chunk) in plane.as_chunks::<2>().0.iter().enumerate() {
                let count = u16::from_le_bytes(*chunk) as u64;
                total += count;
                if bin < 32 {
                    dark += count;
                } else if bin >= 224 {
                    bright += count;
                }
            }
        }
    }
    let pct = |n: u64| n.saturating_mul(100).checked_div(total).unwrap_or(0) as u8;
    if let Ok(mut trace) = TRACE.lock() {
        if trace.len() == TRACE_LEN {
            trace.pop_front();
        }
        trace.push_back(TraceEntry {
            t_us: epoch.elapsed().as_micros() as u64,
            luma,
            raw_changed,
            dark_pct: pct(dark),
            bright_pct: pct(bright),
        });
    }
}
/// Public path used in log messages (same device).
pub const DEVICE_PATH_PUBLIC: &str = DEVICE_PATH;
const ZONES: usize = 16;
/// Per-zone record: 4-byte header + 3 planes x 256 bins x u16.
const RECORD_SIZE: usize = 0x604;
const BINS: usize = 256;

// Private V4L2 command interface on /dev/video60 (from libvtcapture disassembly).
const IOCTL_GET: libc::c_ulong = 0xC0185647;
const IOCTL_SET: libc::c_ulong = 0xC0185648;
const CMD_START: u32 = 0x00982a14;
const CMD_GET_DATA: u32 = 0x00982a16;
const CMD_STOP: u32 = 0x00982a17;
// `VtHistogram::setPresetMode` (command 0x00982a15, presets 0..=3, library zone
// table {16, 16, 16, 14}) is deliberately not used: measured on the C5, every
// preset refreshes at the same ~10 Hz and preset 3's 14-zone layout misplaces colours.

/// Fixed pixel-sample counts per zone per read; used to detect a stalled or
/// blank histogram source (all-zero payload) and for area sanity checks.
/// Values measured on webOS 25; they are content-independent constants.
const ZONE_SAMPLES: [u32; ZONES] = [
    7200, 1728, 2304, 864, 7200, 1380, 1504, 1584, 7200, 1440, 1504, 864, 864, 1584, 704, 720,
];

/// Measured fraction of the histogram range for full-screen white on each
/// channel (231.3/255 = 0.907). Dividing by this recovers encoded sRGB levels
/// while keeping neutrals neutral. Primaries are TV-gamut-compressed upstream;
/// users correct residual saturation with `saturation_boost`.
const WHITE_RESPONSE: f32 = 231.3 / 255.0;

/// Fitted spatial coverage of each histogram zone, normalized fractional
/// screen coordinates (x0, x1, y0, y1). Zones overlap: three full-height
/// halves/columns (z0, z4, z8), a soft center square (z2), a wide bottom strip
/// (z1), and small corner/edge patches (z3, z5..z7, z9..z15).
const ZONE_RECTS: [(f32, f32, f32, f32); ZONES] = [
    (0.225, 0.750, 0.025, 0.975), // z0: center column, 52.5% wide
    (0.200, 0.800, 0.750, 0.950), // z1: wide bottom strip
    (0.275, 0.700, 0.300, 0.675), // z2: center square
    (0.100, 0.400, 0.000, 0.200), // z3: top-left patch
    (0.000, 0.500, 0.000, 1.000), // z4: left half
    (0.000, 0.275, 0.625, 0.975), // z5: bottom-left patch
    (0.000, 0.150, 0.125, 0.825), // z6: left edge strip
    (0.000, 0.275, 0.075, 0.475), // z7: upper-left patch
    (0.500, 1.000, 0.000, 1.000), // z8: right half
    (0.725, 0.975, 0.550, 0.950), // z9: bottom-right patch
    (0.750, 0.900, 0.125, 0.825), // z10: right edge strip
    (0.325, 0.625, 0.000, 0.200), // z11: top-center patch
    (0.575, 0.975, 0.000, 0.150), // z12: top-right strip
    (0.700, 0.975, 0.000, 0.400), // z13: upper-right patch
    (0.300, 0.700, 0.000, 0.125), // z14: top-center narrow strip
    (0.000, 0.075, 0.150, 0.825), // z15: left edge strip
];

/// Outer ioctl argument: magic word, request count, two reserved words, and a
/// pointer to the 20-byte inner command (nested V4L2-style two-level payload).
#[repr(C)]
struct OuterArg {
    magic: u32,
    count: u32,
    _r0: u32,
    _r1: u32,
    _r2: u32,
    inner_ptr: *mut InnerArg,
}

/// Inner command: command magic, payload size (0 for control commands), two
/// reserved words, and for GET_DATA the output pointer that the kernel fills
/// with `ZONES * RECORD_SIZE` bytes of histograms.
#[repr(C)]
struct InnerArg {
    cmd: u32,
    size: u32,
    _r0: u32,
    out_ptr: u32,
    _r1: u32,
}

pub struct HistogramCapture {
    fd: RawFd,
    width: u32,
    height: u32,
    /// Synthetic BGRA frame: one pixel per histogram zone neighborhood (4x4);
    /// the downstream ZoneSampler maps its configured light zones onto it.
    frame: Vec<u8>,
    started: bool,
    last_read: Instant,
    read_interval: Duration,
    /// Cached per-zone colors from the last histogram read (BGRA byte order).
    zone_colors: [[u8; 4]; ZONES],
    /// Per-plane bin totals from the last read; used for the layout probe.
    last_totals: [[u32; 3]; ZONES],
    empty_reads: u32,
    epoch: Instant,
}

impl HistogramCapture {
    /// Cheap capability check for capture-source policy: on this device the
    /// aggregate backend is the primary capture path (see `create_capture`),
    /// so non-HDMI foreground apps must not block the pipeline. `try_new`
    /// still verifies the real hardware contract; if it fails and a frame
    /// backend takes over, source-pause semantics return with it.
    pub fn device_present() -> bool {
        std::fs::metadata(DEVICE_PATH).is_ok()
    }

    /// Opens `/dev/video60` and starts histogram collection.
    pub fn try_new(target_width: u32, target_height: u32) -> Result<Self> {
        if target_width == 0 || target_height == 0 {
            return Err(anyhow!("invalid capture dimensions"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(DEVICE_PATH)
            .with_context(|| format!("open {}", DEVICE_PATH))?;
        let fd = std::os::unix::io::IntoRawFd::into_raw_fd(file);
        let mut capture = Self {
            fd,
            width: target_width,
            height: target_height,
            frame: vec![0; Self::FRAME_W as usize * Self::FRAME_H as usize * 4],
            started: false,
            last_read: Instant::now() - Duration::from_secs(1),
            // Measured on the C5 (webOS 10.3.1): the histogram engine refreshes
            // about every 100 ms (~10 Hz; see /api/histogram/trace). Polling at
            // ~60 Hz does not raise that rate, but picks each refresh up within
            // one 16 ms tick, which keeps added latency minimal.
            read_interval: Duration::from_millis(16),
            zone_colors: [[0, 0, 0, 255]; ZONES],
            last_totals: [[0; 3]; ZONES],
            empty_reads: 0,
            epoch: Instant::now(),
        };
        capture.start().context("start histogram capture")?;
        // Layout probe: the zone geometry and sample-count constants were
        // calibrated on webOS 24/25 hardware. If this device reports a
        // different fixed total per zone, the layout constants would be wrong
        // for it — fail here so create_capture falls back to another backend.
        // The first refresh after START can be partial (~5400 of 7200 samples in
        // zone 0), so give the engine up to ~200 ms to settle before judging it.
        let mut layout = Err(anyhow!(
            "histogram probe read failed on {}",
            DEVICE_PATH_PUBLIC
        ));
        for attempt in 0..10 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(20));
            }
            if !capture.read_histograms() {
                continue;
            }
            layout = capture.check_layout();
            if layout.is_ok() {
                break;
            }
        }
        layout?;
        info!("[+] Histogram layout verified (16 zones, sample counts match calibration).");
        Ok(capture)
    }

    /// Sample counts per zone must match the calibrated 16-zone layout.
    fn check_layout(&self) -> Result<()> {
        for (z, totals) in self.last_totals.iter().enumerate() {
            let expected = ZONE_SAMPLES[z];
            let measured = totals[0];
            if expected == 0 || measured == 0 {
                return Err(anyhow!(
                    "histogram zone {z} reported no samples (expected {expected})"
                ));
            }
            let drift = (measured as f32 - expected as f32).abs() / expected as f32;
            if drift > 0.05 {
                return Err(anyhow!(
                    "histogram zone {z} sample count {measured} deviates {:.0}% \
                     from calibrated {expected}; unknown histogram layout",
                    drift * 100.0
                ));
            }
        }
        Ok(())
    }

    const FRAME_W: u32 = 4;
    const FRAME_H: u32 = 4;

    fn start(&mut self) -> Result<()> {
        let ret = self.control(CMD_START, 0, 0)?;
        if ret < 0 {
            return Err(anyhow!("histogram START ioctl failed with {}", ret));
        }
        self.started = true;
        info!(
            "[+] Histogram capture started on {} ({} zones)",
            DEVICE_PATH, ZONES
        );
        Ok(())
    }

    /// Sends a control command (START/STOP) through the nested ioctl.
    fn control(&self, cmd: u32, size: u32, out_ptr: u32) -> Result<i32> {
        let mut inner = InnerArg {
            cmd,
            size,
            _r0: 0,
            out_ptr,
            _r1: 0,
        };
        let mut outer = OuterArg {
            magic: 0x00980000,
            count: 1,
            _r0: 0,
            _r1: 0,
            _r2: 0,
            inner_ptr: &mut inner,
        };
        // SAFETY: both args are repr(C), correctly sized, and pointers passed to
        // this ioctl remain valid for the call's duration.
        let ret = unsafe { libc::ioctl(self.fd, IOCTL_SET, &mut outer as *mut OuterArg) };
        if ret < 0 {
            return Err(anyhow::Error::from(std::io::Error::last_os_error())
                .context(format!("histogram control ioctl cmd=0x{cmd:08x} failed")));
        }
        Ok(ret)
    }

    /// GET_DATA into a local buffer, then decode into `zone_colors`.
    /// Returns false on transient failure (device not ready yet).
    fn read_histograms(&mut self) -> bool {
        let mut payload = [0u8; ZONES * RECORD_SIZE];
        let mut inner = InnerArg {
            cmd: CMD_GET_DATA,
            size: (ZONES * RECORD_SIZE) as u32,
            _r0: 0,
            out_ptr: payload.as_mut_ptr() as u32,
            _r1: 0,
        };
        let mut outer = OuterArg {
            magic: 0x00980000,
            count: 1,
            _r0: 0,
            _r1: 0,
            _r2: 0,
            inner_ptr: &mut inner,
        };
        // SAFETY: repr(C) args, out_ptr points at the payload buffer which
        // outlives the ioctl call.
        let ret = unsafe { libc::ioctl(self.fd, IOCTL_GET, &mut outer as *mut OuterArg) };
        if ret < 0 {
            return false;
        }
        let ok = self.decode_payload(&payload);
        if ok {
            trace_read(&payload, &self.zone_colors, self.epoch);
        }
        ok
    }

    /// Converts 16 zone records into per-zone BGRA colors. Each record is a
    /// 4-byte header followed by three 256-bin u16 histograms (R, G, B).
    fn decode_payload(&mut self, payload: &[u8]) -> bool {
        if payload.len() < ZONES * RECORD_SIZE {
            return false;
        }
        let mut any_nonempty = false;
        for (z, colors) in self.zone_colors.iter_mut().enumerate() {
            let rec = &payload[z * RECORD_SIZE..(z + 1) * RECORD_SIZE];
            // Per-plane totals: all three planes sample the same pixels, but
            // decoding each plane's own total is robust to partial updates.
            let mut totals = [0u32; 3];
            let mut sums = [0.0f32; 3];
            for plane in 0..3 {
                let base = 4 + plane * BINS * 2;
                for (bin, chunk) in rec[base..base + BINS * 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .enumerate()
                {
                    let count = u16::from_le_bytes(*chunk) as u32;
                    totals[plane] += count;
                    sums[plane] += (bin as u32 * count) as f32;
                }
            }
            if totals.contains(&0) {
                // Keep last color for transient glitches; caller tracks streaks.
                continue;
            }
            self.last_totals[z] = totals;
            // Weighted bin mean -> channel level; normalize to encoded sRGB
            // with the measured white response (which is itself on the 0..255
            // scale: 231.3/255 = 0.907, so dividing by it restores full
            // scale). Bin index is the value scale.
            let r = (sums[0] / totals[0] as f32) / WHITE_RESPONSE;
            let g = (sums[1] / totals[1] as f32) / WHITE_RESPONSE;
            let b = (sums[2] / totals[2] as f32) / WHITE_RESPONSE;
            *colors = [
                b.clamp(0.0, 255.0) as u8,
                g.clamp(0.0, 255.0) as u8,
                r.clamp(0.0, 255.0) as u8,
                255,
            ];
            any_nonempty = true;
        }
        any_nonempty
    }

    /// Repaints the synthetic 4x4 BGRA frame from zone colors. Each frame
    /// pixel takes the color of the finest (smallest-area) zone whose
    /// rectangle covers that pixel's center.
    fn paint_frame(&mut self) {
        for py in 0..Self::FRAME_H {
            for px in 0..Self::FRAME_W {
                let cx = (px as f32 + 0.5) / Self::FRAME_W as f32;
                let cy = (py as f32 + 0.5) / Self::FRAME_H as f32;
                let mut best: Option<(usize, f32)> = None;
                for (z, &(x0, x1, y0, y1)) in ZONE_RECTS.iter().enumerate() {
                    if cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1 {
                        let area = (x1 - x0) * (y1 - y0);
                        if best.is_none_or(|(_, a)| area < a) {
                            best = Some((z, area));
                        }
                    }
                }
                let color = match best {
                    Some((z, _)) => self.zone_colors[z],
                    None => [0, 0, 0, 255],
                };
                let offset = ((py * Self::FRAME_W + px) as usize) * 4;
                self.frame[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }

    fn stop(&mut self) {
        if self.started {
            let _ = self.control(CMD_STOP, 0, 0);
            self.started = false;
            info!("[-] Histogram capture stopped.");
        }
    }
}

impl ScreenCapture for HistogramCapture {
    fn acquire_frame(&mut self) -> std::result::Result<CapturedFrame<'_>, CaptureError> {
        if !self.started {
            return Err(CaptureError::RestartRequired {
                backend: "vthistogram",
                reason: "histogram stream stopped",
            });
        }
        FRAME_REQUESTS.fetch_add(1, Ordering::Relaxed);
        // Throttle reads to the histogram refresh cadence.
        if self.last_read.elapsed() >= self.read_interval {
            let before = self.zone_colors;
            let started = Instant::now();
            let ok = self.read_histograms();
            let nanos = started.elapsed().as_nanos() as u64;
            READ_NS_TOTAL.fetch_add(nanos, Ordering::Relaxed);
            READ_NS_MAX.fetch_max(nanos, Ordering::Relaxed);
            if ok {
                READS.fetch_add(1, Ordering::Relaxed);
                if self.zone_colors != before {
                    CHANGED_READS.fetch_add(1, Ordering::Relaxed);
                }
                self.empty_reads = 0;
            } else {
                FAILED_READS.fetch_add(1, Ordering::Relaxed);
                self.empty_reads = self.empty_reads.saturating_add(1);
            }
            self.last_read = Instant::now();
        }
        // Unresponsive device: pause via restart, recovery logic backs off.
        if self.empty_reads >= 20 {
            self.stop();
            return Err(CaptureError::RestartRequired {
                backend: "vthistogram",
                reason: "histogram reads unavailable",
            });
        }
        self.paint_frame();
        Ok(CapturedFrame {
            data: &self.frame,
            width: Self::FRAME_W,
            height: Self::FRAME_H,
            is_bgra: true,
        })
    }

    fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn is_real_hardware(&self) -> bool {
        true
    }

    fn fixed_cadence(&self) -> bool {
        true
    }
}

impl Drop for HistogramCapture {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only instance with no device attached (fd -1, never touched by
    /// the decode/paint paths under test).
    fn test_capture() -> HistogramCapture {
        HistogramCapture {
            fd: -1,
            width: 160,
            height: 90,
            frame: vec![
                0;
                HistogramCapture::FRAME_W as usize * HistogramCapture::FRAME_H as usize * 4
            ],
            started: false,
            last_read: Instant::now(),
            read_interval: Duration::from_millis(66),
            zone_colors: [[0, 0, 0, 255]; ZONES],
            last_totals: [[0; 3]; ZONES],
            empty_reads: 0,
            epoch: Instant::now(),
        }
    }

    /// Serializes 3x256 u16 bins + 4-byte header into a zone record.
    fn zone_record(bins: [[u16; BINS]; 3]) -> [u8; RECORD_SIZE] {
        let mut rec = [0u8; RECORD_SIZE];
        for (plane, bins) in bins.iter().enumerate() {
            for (bin, &count) in bins.iter().enumerate() {
                let offset = 4 + plane * BINS * 2 + bin * 2;
                rec[offset..offset + 2].copy_from_slice(&count.to_le_bytes());
            }
        }
        rec
    }

    fn decode(capture: &mut HistogramCapture, payload: &[u8]) -> bool {
        capture.decode_payload(payload)
    }

    #[test]
    fn white_histogram_decodes_to_full_scale_neutral() {
        let mut capture = test_capture();
        // All bins pile up around the measured white response level (231).
        let mut bins = [0u16; BINS];
        bins[230] = 1000;
        bins[231] = 1000;
        bins[232] = 1000;
        let mut payload = Vec::new();
        for _ in 0..ZONES {
            payload.extend_from_slice(&zone_record([bins; 3]));
        }
        assert!(decode(&mut capture, &payload));
        for colors in capture.zone_colors {
            // ~231/0.907 = ~255 after white-point normalization.
            assert!(colors[0] > 250, "B {}", colors[0]);
            assert!(colors[1] > 250, "G {}", colors[1]);
            assert!(colors[2] > 250, "R {}", colors[2]);
            assert_eq!(colors[3], 255);
        }
    }

    #[test]
    fn red_histogram_decodes_with_correct_channel_order() {
        let mut capture = test_capture();
        // Plane 0 (R) at bin 197, planes 1/2 near zero (TV gamut-compressed red).
        let mut r = [0u16; BINS];
        r[197] = 1000;
        let mut g = [0u16; BINS];
        g[38] = 500;
        let mut b = [0u16; BINS];
        b[43] = 500;
        let mut payload = Vec::new();
        for _ in 0..ZONES {
            payload.extend_from_slice(&zone_record([r, g, b]));
        }
        assert!(decode(&mut capture, &payload));
        let color = capture.zone_colors[0];
        // BGRA order: [0]=B, [1]=G, [2]=R.
        assert!(color[2] > 200, "R {}", color[2]);
        assert!(color[1] < 60, "G {}", color[1]);
        assert!(color[0] < 60, "B {}", color[0]);
    }

    #[test]
    fn empty_payload_keeps_previous_colors() {
        let mut capture = test_capture();
        let mut bins = [0u16; BINS];
        bins[100] = 10;
        let mut payload = Vec::new();
        for _ in 0..ZONES {
            payload.extend_from_slice(&zone_record([bins; 3]));
        }
        assert!(decode(&mut capture, &payload));
        assert_eq!(capture.zone_colors[0][2], (100.0 / WHITE_RESPONSE) as u8);
        // All-zero payload: totals are zero, colors unchanged.
        let zeroed = vec![0u8; ZONES * RECORD_SIZE];
        assert!(!decode(&mut capture, &zeroed));
        assert_eq!(capture.zone_colors[0][2], (100.0 / WHITE_RESPONSE) as u8);
    }

    #[test]
    fn layout_check_accepts_calibration_and_reports_drift_in_percent() {
        let mut capture = test_capture();
        for (z, totals) in capture.last_totals.iter_mut().enumerate() {
            *totals = [ZONE_SAMPLES[z]; 3];
        }
        assert!(capture.check_layout().is_ok());
        // A partial first refresh after START, as seen on the C5.
        capture.last_totals[0] = [5400; 3];
        let error = capture.check_layout().unwrap_err().to_string();
        assert!(error.contains("deviates 25%"), "{error}");
        capture.last_totals[0] = [ZONE_SAMPLES[0]; 3];
        capture.last_totals[14] = [0; 3];
        assert!(capture.check_layout().is_err());
    }

    #[test]
    fn undersized_payload_is_rejected() {
        let mut capture = test_capture();
        assert!(!decode(&mut capture, &[0u8; RECORD_SIZE * (ZONES - 1)]));
    }

    #[test]
    fn zone_rects_match_measured_sample_counts() {
        // Area from the fitted rectangles must approximate the fixed sample
        // fraction (samples / 14400 full-screen density).
        for (z, (x0, x1, y0, y1)) in ZONE_RECTS.iter().enumerate() {
            let area = (x1 - x0) * (y1 - y0);
            let measured = ZONE_SAMPLES[z] as f32 / 14400.0;
            assert!(
                (area - measured).abs() < 0.03,
                "zone {z}: fitted area {area:.3}, measured {measured:.3}"
            );
        }
    }

    #[test]
    fn paint_frame_uses_finest_covering_zone() {
        let mut capture = test_capture();
        for z in 0..ZONES {
            capture.zone_colors[z] = [z as u8, 0, z as u8, 255];
        }
        capture.paint_frame();
        // Frame pixel (1,1) center = (0.375, 0.375): covered by z0 (center
        // column, area 0.4775), z2 (center square), and z4 (left half).
        // z2 is the smallest covering rect (0.425 x 0.375 = 0.159 area).
        let offset = (HistogramCapture::FRAME_W + 1) as usize * 4;
        assert_eq!(capture.frame[offset], 2, "B channel holds zone id");
    }

    #[test]
    fn paint_frame_covers_every_pixel() {
        let mut capture = test_capture();
        for colors in capture.zone_colors.iter_mut() {
            *colors = [255, 255, 255, 255];
        }
        capture.paint_frame();
        // The 16 zone rectangles overlap enough to cover the full screen
        // union (halves z4/z8 alone cover everything); every pixel must get
        // a zone color, never the fallback black.
        for chunk in capture.frame.as_chunks::<4>().0 {
            assert_eq!(chunk, &[255, 255, 255, 255]);
        }
    }
}
