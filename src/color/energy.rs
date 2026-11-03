//! Dynamic response: scale smoothing with how much the picture is moving.
//!
//! `EnergyMeter` turns frame-to-frame change into an energy level in 0..1:
//! calm pans and breakdowns sit low, drops with fast cuts, flashing stage
//! lights and strobes sit high. It rises quickly (a drop should hit at once)
//! and falls slowly (the room eases back into calm). `DynamicParams` maps the
//! level onto the same knobs the fixed response modes use, interpolating from
//! a gentle, Default-like response to Instant.
//!
//! The meter measures change *per second*, so it reads the same whether the
//! source refreshes at ~10 Hz (C5 histogram) or 60 Hz (frame capture).

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Coarse grid the frame is reduced to before comparing (4x4 = histogram frame).
const GRID: usize = 4;
/// Change accumulates with this time constant: roughly "change over the last ~0.7 s".
const INTEGRATION: Duration = Duration::from_millis(700);
/// Change rate (mean per-cell max-channel delta, 0..1, per second) that reads as calm…
const CALM_RATE: f32 = 0.12;
/// …and as full energy. Tune against real footage with `/api/status` → `energy`.
const PEAK_RATE: f32 = 1.6;
/// How fast the reported level follows: quick attack, slow release.
const ATTACK: Duration = Duration::from_millis(120);
const RELEASE: Duration = Duration::from_millis(1000);

/// Latest level for `/api/status` (f32 bits).
static LEVEL_BITS: AtomicU32 = AtomicU32::new(0);

pub fn current_level() -> f32 {
    f32::from_bits(LEVEL_BITS.load(Ordering::Relaxed))
}

#[derive(Debug, Default)]
pub struct EnergyMeter {
    previous: Option<[[f32; 3]; GRID * GRID]>,
    /// Leaky integral of change; divided by the time constant it is a rate.
    accumulated: f32,
    level: f32,
    last: Option<Instant>,
}

impl EnergyMeter {
    pub fn reset(&mut self) {
        *self = Self::default();
        LEVEL_BITS.store(0f32.to_bits(), Ordering::Relaxed);
    }

    /// Feed every frame (4 bytes per pixel). `boost` forces full energy, e.g.
    /// while strobe boost is replaying a detected strobe. Returns the level.
    pub fn observe(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        is_bgra: bool,
        boost: bool,
        now: Instant,
    ) -> f32 {
        let dt = self
            .last
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last))
            .min(Duration::from_millis(250));
        let grid = reduce(data, width, height, is_bgra);
        let change = match (&self.previous, &grid) {
            (Some(previous), Some(grid)) => mean_change(previous, grid),
            _ => 0.0,
        };
        if grid.is_some() {
            self.previous = grid;
        }
        let decay = (-dt.as_secs_f32() / INTEGRATION.as_secs_f32()).exp();
        self.accumulated = self.accumulated * decay + change;
        let rate = self.accumulated / INTEGRATION.as_secs_f32();
        let target = if boost {
            1.0
        } else {
            ((rate - CALM_RATE) / (PEAK_RATE - CALM_RATE)).clamp(0.0, 1.0)
        };
        let tau = if target > self.level { ATTACK } else { RELEASE };
        let follow = 1.0 - (-dt.as_secs_f32() / tau.as_secs_f32()).exp();
        self.level += (target - self.level) * follow;
        LEVEL_BITS.store(self.level.to_bits(), Ordering::Relaxed);
        self.level
    }
}

/// Average the frame into a GRID x GRID array of normalised RGB.
fn reduce(data: &[u8], width: u32, height: u32, is_bgra: bool) -> Option<[[f32; 3]; GRID * GRID]> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || data.len() < w * h * 4 {
        return None;
    }
    let mut sums = [[0f32; 3]; GRID * GRID];
    let mut counts = [0u32; GRID * GRID];
    // Stride keeps large captures cheap; small frames use every pixel.
    let step_x = (w / 32).max(1);
    let step_y = (h / 18).max(1);
    for y in (0..h).step_by(step_y) {
        for x in (0..w).step_by(step_x) {
            let cell = (y * GRID / h) * GRID + x * GRID / w;
            let p = &data[(y * w + x) * 4..(y * w + x) * 4 + 3];
            let (r, g, b) = if is_bgra {
                (p[2], p[1], p[0])
            } else {
                (p[0], p[1], p[2])
            };
            sums[cell][0] += r as f32;
            sums[cell][1] += g as f32;
            sums[cell][2] += b as f32;
            counts[cell] += 1;
        }
    }
    let mut grid = [[0f32; 3]; GRID * GRID];
    for (cell, (sum, count)) in grid.iter_mut().zip(sums.iter().zip(counts)) {
        if count > 0 {
            for c in 0..3 {
                cell[c] = sum[c] / count as f32 / 255.0;
            }
        }
    }
    Some(grid)
}

/// Mean over cells of the largest channel change (0..1).
fn mean_change(a: &[[f32; 3]; GRID * GRID], b: &[[f32; 3]; GRID * GRID]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (0..3).map(|c| (x[c] - y[c]).abs()).fold(0.0, f32::max))
        .sum::<f32>()
        / (GRID * GRID) as f32
}

/// Response knobs for one energy level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynamicParams {
    pub smoothing: f32,
    pub max_color_step: u8,
    pub gentle_room_lights: bool,
    pub instant_scene_cuts: bool,
    /// When false, the black-frame hold is suspended so dark flashes show.
    pub black_hold: bool,
}

/// Smoothing at zero energy: between Slow (0.2) and Default (0.35).
const CALM_SMOOTHING: f32 = 0.25;

impl DynamicParams {
    /// `previous` gives hysteresis to the on/off switches so they don't chatter.
    pub fn for_level(level: f32, previous: Option<DynamicParams>) -> Self {
        let level = level.clamp(0.0, 1.0);
        let smoothing = CALM_SMOOTHING + (1.0 - CALM_SMOOTHING) * level;
        // Step limit opens geometrically 8 → 64, then lifts entirely near the top.
        let max_color_step = if level >= 0.85 {
            u8::MAX
        } else {
            (8.0f32 * 8.0f32.powf(level / 0.85))
                .round()
                .clamp(8.0, 64.0) as u8
        };
        let switch = |on_above: f32, was: Option<bool>| match was {
            Some(true) => level > on_above - 0.08,
            _ => level > on_above,
        };
        Self {
            smoothing,
            max_color_step,
            gentle_room_lights: !switch(0.4, previous.map(|p| !p.gentle_room_lights)),
            instant_scene_cuts: switch(0.65, previous.map(|p| p.instant_scene_cuts)),
            black_hold: !switch(0.5, previous.map(|p| !p.black_hold)),
        }
    }

    /// Worth pushing to the samplers (avoids churn every frame).
    pub fn differs(&self, other: &DynamicParams) -> bool {
        (self.smoothing - other.smoothing).abs() >= 0.01
            || self.max_color_step != other.max_color_step
            || self.gentle_room_lights != other.gentle_room_lights
            || self.instant_scene_cuts != other.instant_scene_cuts
            || self.black_hold != other.black_hold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_micros(16_667);

    fn frame(cells: impl Fn(usize) -> u8) -> Vec<u8> {
        (0..16)
            .flat_map(|i| {
                let v = cells(i);
                [v, v, v, 255]
            })
            .collect()
    }

    /// Run `seconds` at 60 Hz with a new picture every `refresh_ticks` (6 ≈ the C5's 10 Hz).
    fn run(seconds: f32, refresh_ticks: usize, picture: impl Fn(usize) -> Vec<u8>) -> Vec<f32> {
        let mut meter = EnergyMeter::default();
        let start = Instant::now();
        let mut current = picture(0);
        (0..(seconds * 60.0) as usize)
            .map(|tick| {
                if tick % refresh_ticks == 0 {
                    current = picture(tick / refresh_ticks);
                }
                meter.observe(&current, 4, 4, true, false, start + TICK * tick as u32)
            })
            .collect()
    }

    fn settled(levels: &[f32]) -> f32 {
        levels[levels.len() - 60..].iter().sum::<f32>() / 60.0
    }

    #[test]
    fn a_slow_pan_reads_calm() {
        // Brightness drifting across the screen by ~1% per refresh.
        let levels = run(6.0, 6, |n| frame(|i| ((i * 8 + n) % 200) as u8 + 20));
        assert!(settled(&levels) < 0.1, "pan energy {}", settled(&levels));
    }

    #[test]
    fn a_static_scene_reads_zero() {
        let levels = run(3.0, 6, |_| frame(|i| (i * 10) as u8));
        assert!(settled(&levels) < 0.01);
    }

    #[test]
    fn a_drop_with_flashing_lights_reads_high_and_rises_fast() {
        // Every refresh flips half the cells between dark and bright.
        let picture = |n: usize| frame(|i| if (i + n).is_multiple_of(2) { 240 } else { 15 });
        let levels = run(3.0, 6, picture);
        assert!(settled(&levels) > 0.9, "drop energy {}", settled(&levels));
        let to_half = levels.iter().position(|l| *l > 0.5).unwrap();
        assert!(to_half <= 30, "reached 0.5 after {to_half} ticks");
    }

    #[test]
    fn rate_is_independent_of_refresh_rate() {
        // The same motion per second, delivered at 10 Hz and at 60 Hz.
        let at_10hz = run(4.0, 6, |n| frame(|i| ((i * 13 + n * 12) % 256) as u8));
        let at_60hz = run(4.0, 1, |n| frame(|i| ((i * 13 + n * 2) % 256) as u8));
        let (a, b) = (settled(&at_10hz), settled(&at_60hz));
        assert!((a - b).abs() < 0.15, "10 Hz {a} vs 60 Hz {b}");
    }

    #[test]
    fn energy_eases_down_slowly_after_the_action_stops() {
        let mut meter = EnergyMeter::default();
        let start = Instant::now();
        let busy = |n: usize| frame(|i| if (i + n).is_multiple_of(2) { 240 } else { 15 });
        let still = frame(|_| 100);
        let mut level = 0.0;
        for tick in 0..120u32 {
            level = meter.observe(
                &busy(tick as usize / 6),
                4,
                4,
                true,
                false,
                start + TICK * tick,
            );
        }
        assert!(level > 0.9);
        // 0.5 s after the action stops it is still fairly high, after 4 s it is calm.
        let mut after = Vec::new();
        for tick in 120..360 {
            after.push(meter.observe(&still, 4, 4, true, false, start + TICK * tick));
        }
        assert!(after[30] > 0.5, "0.5 s later {}", after[30]);
        assert!(
            *after.last().unwrap() < 0.15,
            "4 s later {}",
            after.last().unwrap()
        );
    }

    #[test]
    fn strobes_force_full_energy() {
        let mut meter = EnergyMeter::default();
        let start = Instant::now();
        let still = frame(|_| 100);
        let mut level = 0.0;
        for tick in 0..60 {
            level = meter.observe(&still, 4, 4, true, true, start + TICK * tick);
        }
        assert!(level > 0.99);
    }

    #[test]
    fn params_run_from_gentle_to_instant() {
        let calm = DynamicParams::for_level(0.0, None);
        assert_eq!(calm.smoothing, CALM_SMOOTHING);
        assert_eq!(calm.max_color_step, 8);
        assert!(calm.gentle_room_lights && calm.black_hold && !calm.instant_scene_cuts);

        let mid = DynamicParams::for_level(0.5, None);
        assert!(mid.smoothing > 0.5 && mid.max_color_step > 12 && mid.max_color_step < 64);

        let peak = DynamicParams::for_level(1.0, None);
        assert_eq!(peak.smoothing, 1.0);
        assert_eq!(peak.max_color_step, u8::MAX);
        assert!(!peak.gentle_room_lights && !peak.black_hold && peak.instant_scene_cuts);

        // Smoothing never decreases as energy rises.
        let mut previous = 0.0;
        for step in 0..=100 {
            let p = DynamicParams::for_level(step as f32 / 100.0, None);
            assert!(p.smoothing >= previous);
            previous = p.smoothing;
        }
    }

    #[test]
    fn switches_have_hysteresis() {
        let on = DynamicParams::for_level(0.7, None);
        assert!(on.instant_scene_cuts);
        // Slightly below the threshold keeps it on; well below turns it off.
        assert!(DynamicParams::for_level(0.62, Some(on)).instant_scene_cuts);
        assert!(!DynamicParams::for_level(0.5, Some(on)).instant_scene_cuts);
        // And from off, slightly above the lower edge stays off.
        let off = DynamicParams::for_level(0.5, None);
        assert!(!DynamicParams::for_level(0.62, Some(off)).instant_scene_cuts);
    }
}
