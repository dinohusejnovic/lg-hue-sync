//! Strobe boost: make each light strobe while its part of the picture strobes.
//!
//! The C5's VtHistogram engine refreshes about every 100 ms (~10 Hz), and each
//! refresh is a single-frame snapshot. Followed through normal smoothing, a
//! video strobe becomes a slow, uneven shimmer. A strobe shows up instead as a
//! light's unsmoothed colour flipping between dim and bright on consecutive
//! refreshes, which ordinary video rarely does several times in a row.
//!
//! Detection is per light and relative: concert footage is often dark, so a
//! strobe can be a jump from 2% to 10% brightness. Three alternating flips
//! within 600 ms start a light strobing (about 0.3 s into the strobe).
//!
//! While strobing, a light alternates between the dimmest and brightest
//! colours its zone showed in the last second, at the rate its flips suggest
//! (capped at the configured strobe rate). A zone flipping on nearly every
//! refresh is strobing at or beyond what 10 snapshots a second can resolve,
//! so it gets the configured rate. All lights share one time base, so lights
//! strobing at the same rate flash together. Smoothing history is never
//! touched, so normal sync resumes cleanly when the strobe ends.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::RgbColor;

/// A flip: the brighter of two consecutive refreshes is at least this many
/// times the dimmer one...
const FLIP_RATIO: f32 = 2.0;
/// ...by at least this much luminance (0..1)...
const FLIP_MIN_SWING: f32 = 0.03;
/// ...and bright enough not to be noise around black.
const FLIP_MIN_PEAK: f32 = 0.05;
/// Flips needed, alternating in direction, within `START_WINDOW`.
const START_FLIPS: usize = 3;
const START_WINDOW: Duration = Duration::from_millis(600);
/// Rate and brightness levels come from this much recent history.
const HISTORY: Duration = Duration::from_millis(1000);
/// Flips on at least this share of refreshes: faster than 10 Hz snapshots
/// can resolve, so strobe at the configured maximum.
const ALIASED_SHARE: f32 = 0.85;
/// Release: new refreshes without a flip, after a short quiet...
const CALM_REFRESHES_TO_RELEASE: u32 = 3;
const CALM_MIN_QUIET: Duration = Duration::from_millis(250);
/// ...or no flip at all for this long. A strobe near the refresh rate can be
/// caught on the same phase, repeating byte-identical refreshes for over a
/// second, so this is longer than the calm release.
const RELEASE: Duration = Duration::from_millis(2000);
/// Backends that cannot report new data fall back to visible change.
const CALM_DELTA: f32 = 0.01;

/// Whether any light is strobing; shown in `/api/status`.
pub static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub const MIN_STROBE_HZ: f32 = 2.0;
/// Hue streams to bulbs at ~25 Hz, so each phase needs at least ~40 ms.
pub const MAX_STROBE_HZ: f32 = 12.5;

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Instant,
    luma: f32,
    color: RgbColor,
}

#[derive(Debug, Default)]
struct LightStrobe {
    last: Option<Sample>,
    /// Refresh samples within `HISTORY`.
    history: VecDeque<Sample>,
    /// Flips within `HISTORY`: (time, brighter than before).
    flips: VecDeque<(Instant, bool)>,
    active: bool,
    calm: u32,
}

impl LightStrobe {
    fn observe(&mut self, color: RgbColor, new_data: Option<bool>, now: Instant) {
        let luma = color.luminance();
        let new_data = new_data.unwrap_or_else(|| {
            self.last
                .is_none_or(|last| (luma - last.luma).abs() > CALM_DELTA)
        });
        if !new_data {
            return;
        }
        if let Some(last) = self.last {
            let (low, high) = (luma.min(last.luma), luma.max(last.luma));
            if high >= FLIP_MIN_PEAK && high - low >= FLIP_MIN_SWING && high >= low * FLIP_RATIO {
                self.flips.push_back((now, luma > last.luma));
                self.calm = 0;
            } else {
                self.calm = self.calm.saturating_add(1);
            }
        }
        let sample = Sample {
            at: now,
            luma,
            color,
        };
        self.last = Some(sample);
        self.history.push_back(sample);
        let expired = |at: Instant| now.saturating_duration_since(at) > HISTORY;
        while self.history.front().is_some_and(|s| expired(s.at)) {
            self.history.pop_front();
        }
        while self.flips.front().is_some_and(|(at, _)| expired(*at)) {
            self.flips.pop_front();
        }
        if !self.active && self.starts(now) {
            self.active = true;
        }
    }

    /// Enough alternating flips close together.
    fn starts(&self, now: Instant) -> bool {
        let recent: Vec<bool> = self
            .flips
            .iter()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= START_WINDOW)
            .map(|(_, up)| *up)
            .collect();
        let alternations = recent.windows(2).filter(|pair| pair[0] != pair[1]).count();
        recent.len() >= START_FLIPS && alternations + 1 >= START_FLIPS
    }

    fn released(&self, now: Instant) -> bool {
        let quiet = self
            .flips
            .back()
            .map_or(Duration::MAX, |(at, _)| now.saturating_duration_since(*at));
        quiet > RELEASE || (quiet > CALM_MIN_QUIET && self.calm >= CALM_REFRESHES_TO_RELEASE)
    }

    /// Flashes per second, from the flips in the history window.
    fn rate(&self, max_hz: f32) -> f32 {
        let (Some(first), Some(last)) = (self.history.front(), self.history.back()) else {
            return max_hz;
        };
        let span = last.at.saturating_duration_since(first.at).as_secs_f32();
        let refreshes = self.history.len().saturating_sub(1).max(1);
        let flips = self.flips.len() as f32;
        if span <= 0.0 || flips >= ALIASED_SHARE * refreshes as f32 {
            return max_hz;
        }
        // Two flips (up and down) per flash.
        (flips / 2.0 / span).clamp(MIN_STROBE_HZ, max_hz)
    }

    /// The dimmest and brightest colours in the history window.
    fn levels(&self) -> (RgbColor, RgbColor) {
        let by_luma = |a: &&Sample, b: &&Sample| a.luma.total_cmp(&b.luma);
        let low = self.history.iter().min_by(by_luma).map(|s| s.color);
        let high = self.history.iter().max_by(by_luma).map(|s| s.color);
        let black = RgbColor::new(0, 0, 0);
        (low.unwrap_or(black), high.unwrap_or(black))
    }
}

/// Strobe boost for one output (Hue or Nanoleaf), one detector per light.
#[derive(Debug)]
pub struct StrobeLights {
    enabled: bool,
    hz: f32,
    lights: Vec<LightStrobe>,
    epoch: Instant,
}

impl Default for StrobeLights {
    fn default() -> Self {
        Self {
            enabled: false,
            hz: 10.0,
            lights: Vec::new(),
            epoch: Instant::now(),
        }
    }
}

impl StrobeLights {
    pub fn configure(&mut self, enabled: bool, hz: f32) {
        self.hz = hz.clamp(MIN_STROBE_HZ, MAX_STROBE_HZ);
        if enabled != self.enabled {
            self.enabled = enabled;
            self.lights.clear();
        }
    }

    pub fn active(&self) -> bool {
        self.lights.iter().any(|light| light.active)
    }

    /// Feed each light's unsmoothed colour (same order as `colors`) and
    /// whether the capture hardware delivered new data since the previous call
    /// (`None` when the backend cannot tell), then replace the colours of
    /// strobing lights for the current phase.
    pub fn apply<'a>(
        &mut self,
        unsmoothed: &[RgbColor],
        new_data: Option<bool>,
        now: Instant,
        colors: impl Iterator<Item = &'a mut RgbColor>,
    ) {
        if !self.enabled {
            return;
        }
        if self.lights.len() != unsmoothed.len() {
            self.lights = (0..unsmoothed.len())
                .map(|_| LightStrobe::default())
                .collect();
        }
        let elapsed = now.saturating_duration_since(self.epoch).as_secs_f32();
        for ((light, &color), output) in self.lights.iter_mut().zip(unsmoothed).zip(colors) {
            light.observe(color, new_data, now);
            if light.active && light.released(now) {
                light.active = false;
                light.flips.clear();
            }
            if light.active {
                let (low, high) = light.levels();
                let bright = (elapsed * light.rate(self.hz)).fract() < 0.5;
                *output = if bright { high } else { low };
            }
        }
    }
}

/// Publish whether any output is strobing (for `/api/status`).
pub fn publish_active(active: bool) {
    ACTIVE.store(active, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_micros(16_667);
    /// Refresh gaps measured on the C5.
    const GAPS_MS: [u64; 8] = [89, 106, 88, 107, 91, 105, 89, 108];

    fn grey(luma: f32) -> RgbColor {
        let v = (luma * 255.0).round() as u8;
        RgbColor::new(v, v, v)
    }

    /// Runs a 60 Hz loop over a ~10 Hz refresh source for `seconds`.
    /// `zones(n)` gives each light's unsmoothed colour for refresh n. Returns
    /// the output colours of every tick, with the tick's time from the start.
    fn run(
        strobe: &mut StrobeLights,
        seconds: f32,
        lights: usize,
        mut zones: impl FnMut(usize) -> Vec<RgbColor>,
    ) -> Vec<(Duration, Vec<RgbColor>)> {
        let start = Instant::now();
        let (mut next_refresh, mut refresh, mut current) = (Duration::ZERO, 0usize, zones(0));
        let mut out = Vec::new();
        let mut t = Duration::ZERO;
        while t.as_secs_f32() < seconds {
            let new_data = t >= next_refresh;
            if new_data {
                current = zones(refresh);
                next_refresh += Duration::from_millis(GAPS_MS[refresh % GAPS_MS.len()]);
                refresh += 1;
            }
            let mut colors = vec![grey(0.3); lights];
            strobe.apply(&current, Some(new_data), start + t, colors.iter_mut());
            out.push((t, colors));
            t += TICK;
        }
        out
    }

    fn strobe_on(hz: f32) -> StrobeLights {
        let mut strobe = StrobeLights::default();
        strobe.configure(true, hz);
        strobe
    }

    /// First tick at which light `index` stops showing the smoothed colour.
    fn started(out: &[(Duration, Vec<RgbColor>)], index: usize) -> Option<Duration> {
        out.iter()
            .find(|(_, colors)| colors[index] != grey(0.3))
            .map(|(t, _)| *t)
    }

    #[test]
    fn only_the_strobing_zone_strobes_between_its_own_levels() {
        let dim = RgbColor::new(10, 4, 20);
        let bright = RgbColor::new(200, 90, 255);
        let mut strobe = strobe_on(10.0);
        let out = run(&mut strobe, 2.0, 2, |n| {
            vec![if n % 2 == 0 { dim } else { bright }, grey(0.4)]
        });
        let start = started(&out, 0).expect("the strobing zone strobes");
        assert!(
            start <= Duration::from_millis(400),
            "started after {start:?}"
        );
        assert_eq!(started(&out, 1), None, "the steady zone is left alone");
        let shown: std::collections::BTreeSet<_> = out
            .iter()
            .filter(|(t, _)| *t > start)
            .map(|(_, c)| (c[0].r, c[0].g, c[0].b))
            .collect();
        assert_eq!(
            shown,
            [(10, 4, 20), (200, 90, 255)].into_iter().collect(),
            "alternates between the zone's own dim and bright colours"
        );
        assert!(strobe.active());
    }

    #[test]
    fn dark_footage_strobes_are_detected_by_relative_jumps() {
        // 2% <-> 10%: small in absolute terms, five times brighter.
        let mut strobe = strobe_on(10.0);
        let out = run(&mut strobe, 1.5, 1, |n| {
            vec![grey(if n % 2 == 0 { 0.02 } else { 0.10 })]
        });
        assert!(started(&out, 0).is_some());
        // Noise near black (1% <-> 3%) is not a strobe.
        let mut strobe = strobe_on(10.0);
        let out = run(&mut strobe, 1.5, 1, |n| {
            vec![grey(if n % 2 == 0 { 0.01 } else { 0.03 })]
        });
        assert_eq!(started(&out, 0), None);
    }

    #[test]
    fn rate_follows_the_strobe_up_to_the_configured_maximum() {
        // Flipping on every refresh: beyond what 10 snapshots/s resolve.
        let mut strobe = strobe_on(8.0);
        run(&mut strobe, 1.5, 1, |n| {
            vec![grey(if n % 2 == 0 { 0.1 } else { 0.8 })]
        });
        assert_eq!(strobe.lights[0].rate(8.0), 8.0);
        // One flash every third refresh (~3.3 Hz).
        let mut strobe = strobe_on(10.0);
        run(&mut strobe, 1.5, 1, |n| {
            vec![grey(if n % 3 == 0 { 0.8 } else { 0.1 })]
        });
        let rate = strobe.lights[0].rate(10.0);
        assert!((2.5..=4.5).contains(&rate), "rate {rate}");
        assert!(strobe.active());
    }

    #[test]
    fn ordinary_video_and_single_cuts_do_not_strobe() {
        let mut strobe = strobe_on(10.0);
        // A slow pulse and a single hard cut.
        let out = run(&mut strobe, 3.0, 2, |n| {
            vec![
                grey(0.3 + 0.1 * (n as f32 * 0.4).sin()),
                grey(if n < 12 { 0.05 } else { 0.7 }),
            ]
        });
        assert_eq!(started(&out, 0), None);
        assert_eq!(started(&out, 1), None);
        assert!(!strobe.active());
    }

    #[test]
    fn strobing_ends_soon_after_the_flashes_stop() {
        let mut strobe = strobe_on(10.0);
        let out = run(&mut strobe, 2.5, 1, |n| {
            vec![grey(if n < 10 {
                [0.1, 0.8][n % 2]
            } else {
                0.4 + 0.001 * n as f32
            })]
        });
        let last = out
            .iter()
            .rev()
            .find(|(_, colors)| colors[0] != grey(0.3))
            .map(|(t, _)| *t)
            .expect("it strobed");
        // Flashes stop at ~1 s; the calm release needs ~250 ms and 3 refreshes.
        assert!(
            last < Duration::from_millis(1600),
            "still strobing at {last:?}"
        );
        assert!(!strobe.active());
    }

    #[test]
    fn disabled_never_strobes() {
        let mut strobe = StrobeLights::default();
        let out = run(&mut strobe, 1.5, 1, |n| vec![grey([0.1, 0.8][n % 2])]);
        assert_eq!(started(&out, 0), None);
    }
}
