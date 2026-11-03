//! Strobe boost: make the lights strobe while the video strobes.
//!
//! The C5's VtHistogram engine refreshes about every 100 ms (~10 Hz), so a
//! video strobe faster than ~5 Hz aliases into a slow, irregular flicker when
//! colours are simply followed. Each refresh is a single-frame snapshot,
//! though, and a strobe makes consecutive refreshes flip between near-black and
//! near-white. Ordinary video almost never does that several times in a row.
//!
//! `StrobeDetector` recognises that pattern from the frame's mean luminance
//! and, while it lasts, emits a square wave at a chosen rate. `StrobeColors`
//! applies it to one output's colour copy: on-phases show the last bright
//! colours, off-phases are black. Smoothing history is never touched, so
//! normal sync resumes cleanly when the strobe ends. The flashes are not
//! phase-locked to individual video flashes (10 Hz snapshots cannot recover
//! that); they start and stop with the strobe.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::RgbColor;

/// Mean-luminance jump (0..1) between consecutive refreshes counted as a flash edge.
const EDGE_SWING: f32 = 0.35;
/// Edges must cluster within this window to count as strobing.
const WINDOW: Duration = Duration::from_millis(800);
const MIN_EDGES: usize = 3;
/// Once active, keep strobing through edge-free stretches: a video strobe near
/// the ~10 Hz refresh rate can be caught on the same phase for over a second,
/// which reads as a perfectly constant frame.
const RELEASE: Duration = Duration::from_millis(2000);
/// A phase-locked strobe repeats byte-identical histograms; real video (even a
/// still-looking stage) delivers new histogram data on nearly every refresh.
/// This many new refreshes without an edge, after a short quiet, ends the strobe.
const CALM_REFRESHES_TO_RELEASE: u32 = 3;
const CALM_MIN_QUIET: Duration = Duration::from_millis(250);
/// Backends that cannot report new data fall back to visible luma drift.
const CALM_DELTA: f32 = 0.01;

/// Whether a strobe is currently being replayed; shown in `/api/status`.
pub static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Frames at least this bright refresh the colours used for on-phases.
const BRIGHT_LUMA: f32 = 0.5;

pub const MIN_STROBE_HZ: f32 = 2.0;
/// Hue streams to bulbs at ~25 Hz, so each phase needs at least ~40 ms.
pub const MAX_STROBE_HZ: f32 = 12.5;

#[derive(Debug)]
pub struct StrobeDetector {
    enabled: bool,
    hz: f32,
    last_luma: Option<f32>,
    edges: VecDeque<Instant>,
    last_edge: Option<Instant>,
    active_since: Option<Instant>,
    calm_changes: u32,
}

impl Default for StrobeDetector {
    fn default() -> Self {
        Self {
            enabled: false,
            hz: 10.0,
            last_luma: None,
            edges: VecDeque::new(),
            last_edge: None,
            active_since: None,
            calm_changes: 0,
        }
    }
}

impl StrobeDetector {
    pub fn configure(&mut self, enabled: bool, hz: f32) {
        self.hz = hz.clamp(MIN_STROBE_HZ, MAX_STROBE_HZ);
        if enabled != self.enabled {
            self.enabled = enabled;
            self.reset();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn reset(&mut self) {
        self.last_luma = None;
        self.edges.clear();
        self.last_edge = None;
        self.active_since = None;
        self.calm_changes = 0;
    }

    /// Feed the frame's mean luminance (0..1) once per loop iteration, plus
    /// whether the capture hardware delivered new data since the previous call
    /// (`None` when the backend cannot tell). Returns `Some(true)` for an
    /// on-phase and `Some(false)` for an off-phase while a strobe is active.
    pub fn observe(&mut self, luma: f32, refreshed: Option<bool>, now: Instant) -> Option<bool> {
        let phase = self.step(luma, refreshed, now);
        ACTIVE.store(phase.is_some(), std::sync::atomic::Ordering::Relaxed);
        phase
    }

    fn step(&mut self, luma: f32, refreshed: Option<bool>, now: Instant) -> Option<bool> {
        if !self.enabled {
            return None;
        }
        // Repeated reads of the same refresh have zero delta, so only real
        // refreshes can produce edges.
        if let Some(previous) = self.last_luma {
            let delta = (luma - previous).abs();
            let new_data = refreshed.unwrap_or(delta > CALM_DELTA);
            if delta >= EDGE_SWING {
                self.edges.push_back(now);
                self.last_edge = Some(now);
                self.calm_changes = 0;
            } else if new_data {
                self.calm_changes = self.calm_changes.saturating_add(1);
            }
        }
        self.last_luma = Some(luma);
        while self
            .edges
            .front()
            .is_some_and(|edge| now.saturating_duration_since(*edge) > WINDOW)
        {
            self.edges.pop_front();
        }
        if self.active_since.is_none() && self.edges.len() >= MIN_EDGES {
            self.active_since = Some(now);
        }
        let since = self.active_since?;
        let quiet = self
            .last_edge
            .map_or(Duration::MAX, |edge| now.saturating_duration_since(edge));
        if quiet > RELEASE
            || (quiet > CALM_MIN_QUIET && self.calm_changes >= CALM_REFRESHES_TO_RELEASE)
        {
            self.active_since = None;
            self.edges.clear();
            return None;
        }
        let cycles = now.saturating_duration_since(since).as_secs_f32() * self.hz;
        Some(cycles.fract() < 0.5)
    }

    pub fn is_bright(luma: f32) -> bool {
        luma >= BRIGHT_LUMA
    }
}

/// Per-output colours for on-phases.
#[derive(Debug, Default)]
pub struct StrobeColors {
    bright: Vec<RgbColor>,
}

impl StrobeColors {
    /// Rewrite `colors` for the current strobe phase. Outside a strobe the
    /// colours pass through and bright frames refresh the on-phase colours.
    pub fn apply<'a>(
        &mut self,
        phase: Option<bool>,
        frame_bright: bool,
        colors: impl ExactSizeIterator<Item = &'a mut RgbColor>,
    ) {
        let colors: Vec<&mut RgbColor> = colors.collect();
        let capture = frame_bright && colors.iter().any(|c| c.r > 0 || c.g > 0 || c.b > 0);
        if capture && (phase.is_none() || phase == Some(true)) {
            self.bright = colors.iter().map(|c| **c).collect();
        }
        let Some(on) = phase else {
            return;
        };
        let white = RgbColor::new(255, 255, 255);
        for (index, color) in colors.into_iter().enumerate() {
            *color = if on {
                self.bright.get(index).copied().unwrap_or(white)
            } else {
                RgbColor::new(0, 0, 0)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_micros(16_667);

    /// What a 60 Hz loop sees from a ~10 Hz histogram source: (mean luma,
    /// whether a refresh delivered new bytes). Refresh gaps repeat the jitter
    /// measured on the C5 (88-107 ms). `frame(n)` gives the luma of refresh n
    /// and whether its bytes differ from the previous refresh.
    fn refreshes(
        seconds: f32,
        mut frame: impl FnMut(f32) -> (f32, bool),
    ) -> Vec<(f32, Option<bool>)> {
        const GAPS_MS: [f32; 8] = [89.0, 106.0, 88.0, 107.0, 91.0, 105.0, 89.0, 108.0];
        let ticks = (seconds * 60.0) as usize;
        let (mut next_refresh, mut gap, mut held) = (0.0f32, 0usize, 0.0f32);
        (0..ticks)
            .map(|tick| {
                let ms = tick as f32 * 1000.0 / 60.0;
                let mut new = false;
                if ms >= next_refresh {
                    let (luma, changed) = frame(ms);
                    held = luma;
                    new = changed;
                    next_refresh += GAPS_MS[gap % GAPS_MS.len()];
                    gap += 1;
                }
                (held, Some(new))
            })
            .collect()
    }

    /// A full-screen black/white video strobe: same-phase refreshes are identical.
    fn histogram_view(strobe_hz: f32, seconds: f32) -> Vec<(f32, Option<bool>)> {
        let mut last = f32::NAN;
        refreshes(seconds, |ms| {
            let luma = if (ms / 1000.0 * strobe_hz).fract() < 0.5 {
                0.98
            } else {
                0.0
            };
            let changed = luma != last;
            last = luma;
            (luma, changed)
        })
    }

    fn run(detector: &mut StrobeDetector, frames: &[(f32, Option<bool>)]) -> Vec<Option<bool>> {
        let start = Instant::now();
        frames
            .iter()
            .enumerate()
            .map(|(i, (luma, refreshed))| {
                detector.observe(*luma, *refreshed, start + TICK * i as u32)
            })
            .collect()
    }

    fn released_after(phases: &[Option<bool>], tail_start: usize) -> usize {
        let after = phases[tail_start..]
            .iter()
            .position(Option::is_none)
            .expect("strobe must release");
        assert!(phases[tail_start + after..].iter().all(Option::is_none));
        after
    }

    #[test]
    fn aliased_strobes_are_detected_and_replayed_at_the_chosen_rate() {
        for video_hz in [6.0, 8.0, 10.0, 12.0] {
            let mut detector = StrobeDetector::default();
            detector.configure(true, 10.0);
            let phases = run(&mut detector, &histogram_view(video_hz, 6.0));
            let active: Vec<bool> = phases[60..].iter().flatten().copied().collect();
            println!(
                "video {video_hz} Hz: active {:.0}% of ticks after the first second",
                active.len() as f32 * 100.0 / (phases.len() - 60) as f32
            );
            assert!(
                active.len() * 10 >= (phases.len() - 60) * 8,
                "{video_hz} Hz strobe should stay detected: {} active ticks",
                active.len()
            );
            // A 10 Hz square wave at 60 Hz: 3 ticks on, 3 off.
            let flips = active.windows(2).filter(|w| w[0] != w[1]).count();
            let per_second = flips as f32 / 2.0 / (active.len() as f32 / 60.0);
            assert!((per_second - 10.0).abs() < 1.5, "{per_second} Hz");
        }
    }

    #[test]
    fn ordinary_video_and_single_cuts_do_not_trigger() {
        let mut detector = StrobeDetector::default();
        detector.configure(true, 10.0);
        // Hard cuts half a second apart (fast editing), otherwise steady.
        let mut frames = Vec::new();
        for luma in [0.2, 0.9, 0.1, 0.8, 0.15] {
            frames.extend(vec![(luma, None); 30]);
        }
        assert!(run(&mut detector, &frames).iter().all(Option::is_none));
    }

    #[test]
    fn strobe_ends_when_the_flashing_stops_and_disabled_means_never() {
        let mut detector = StrobeDetector::default();
        detector.configure(true, 10.0);
        let mut frames = histogram_view(6.0, 2.0);
        // A frozen frame (no new histogram bytes) is indistinguishable from a
        // phase-locked strobe, so it keeps strobing for up to RELEASE (2 s).
        frames.extend(vec![(0.6, Some(false)); 150]);
        let phases = run(&mut detector, &frames);
        assert!(phases[60..120].iter().any(Option::is_some));
        assert!(phases[phases.len() - 20..].iter().all(Option::is_none));

        let mut off = StrobeDetector::default();
        off.configure(false, 10.0);
        let lumas = histogram_view(10.0, 2.0);
        assert!(run(&mut off, &lumas).iter().all(Option::is_none));
    }

    #[test]
    fn a_still_looking_scene_releases_within_about_400ms() {
        // After the strobe, a stage that barely changes in mean brightness but
        // still delivers new histogram bytes every refresh (motion, noise).
        let mut detector = StrobeDetector::default();
        detector.configure(true, 10.0);
        let mut frames = histogram_view(6.0, 2.0);
        let tail_start = frames.len();
        frames.extend(refreshes(2.0, |_| (0.3, true)));
        let after = released_after(&run(&mut detector, &frames), tail_start);
        println!("still scene released after ~{} ms", after * 1000 / 60);
        assert!(
            after <= 25,
            "released after {after} ticks (~{} ms)",
            after * 1000 / 60
        );
    }

    #[test]
    fn frame_backends_without_refresh_info_release_on_luma_drift() {
        let mut detector = StrobeDetector::default();
        detector.configure(true, 10.0);
        let mut frames: Vec<(f32, Option<bool>)> = histogram_view(6.0, 2.0)
            .into_iter()
            .map(|(luma, _)| (luma, None))
            .collect();
        let tail_start = frames.len();
        frames.extend((0..120).map(|tick| (0.5 + (tick / 6) as f32 * 0.02, None)));
        let after = released_after(&run(&mut detector, &frames), tail_start);
        assert!(after <= 50, "released after {after} ticks");
    }

    #[test]
    fn rate_is_clamped_to_what_hue_can_show() {
        let mut detector = StrobeDetector::default();
        detector.configure(true, 40.0);
        assert_eq!(detector.hz, MAX_STROBE_HZ);
        detector.configure(true, 0.5);
        assert_eq!(detector.hz, MIN_STROBE_HZ);
    }

    #[test]
    fn colours_flash_between_the_last_bright_frame_and_black() {
        let mut strobe = StrobeColors::default();
        let warm = RgbColor::new(250, 180, 90);
        let mut colors = vec![warm, warm];
        strobe.apply(None, true, colors.iter_mut());
        assert_eq!(colors, vec![warm, warm], "passthrough outside a strobe");

        let mut dark = vec![RgbColor::new(3, 3, 3); 2];
        strobe.apply(Some(true), false, dark.iter_mut());
        assert_eq!(
            dark,
            vec![warm, warm],
            "on-phase replays the bright colours"
        );
        strobe.apply(Some(false), false, dark.iter_mut());
        assert_eq!(dark, vec![RgbColor::new(0, 0, 0); 2]);

        // With no bright frame yet, on-phases fall back to white.
        let mut fresh = StrobeColors::default();
        let mut one = vec![RgbColor::new(0, 0, 0)];
        fresh.apply(Some(true), false, one.iter_mut());
        assert_eq!(one, vec![RgbColor::new(255, 255, 255)]);
    }
}
