use super::HueDtlsClient;
use anyhow::{anyhow, Result};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    packet: Option<Vec<u8>>,
    /// A packet newer than the last one sent; it goes out at the minimum gap.
    fresh: bool,
    error: Option<String>,
    stopped: bool,
}

/// Keep-alive repeat of an unchanged packet at Hue's recommended ~50 Hz.
const REPEAT_INTERVAL: Duration = Duration::from_millis(20);
/// New colours go out as soon as this gap since the previous *new* colours allows
/// (~60 Hz cap). A keep-alive repeat does not delay them: with histogram capture
/// new colours arrive ~10 times a second, and waiting behind a repeat would add
/// up to 16.5 ms of latency for nothing.
const MIN_SEND_GAP: Duration = Duration::from_micros(16_500);

fn send_due(last_sent: Instant, last_fresh: Instant, fresh: bool) -> Instant {
    if fresh {
        last_fresh + MIN_SEND_GAP
    } else {
        last_sent + REPEAT_INTERVAL
    }
}

/// Repeat cached output at Hue's recommended cadence without resampling static colours.
pub struct RepeatingHueStream {
    state: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl RepeatingHueStream {
    pub fn new(mut client: HueDtlsClient) -> Result<Self> {
        Self::with_sender(move |packet| client.send(packet))
    }

    fn with_sender(mut send: impl FnMut(&[u8]) -> Result<()> + Send + 'static) -> Result<Self> {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let shared = state.clone();
        let worker = thread::Builder::new()
            .name("hue-output".into())
            .spawn(move || {
                let (lock, wake) = &*shared;
                let mut last_sent = Instant::now() - REPEAT_INTERVAL;
                let mut last_fresh = last_sent;
                loop {
                    let mut state = lock.lock().unwrap();
                    loop {
                        if state.stopped {
                            break;
                        }
                        if state.packet.is_none() {
                            state = wake.wait(state).unwrap();
                            continue;
                        }
                        let due = send_due(last_sent, last_fresh, state.fresh);
                        let now = Instant::now();
                        if now >= due {
                            break;
                        }
                        state = wake.wait_timeout(state, due - now).unwrap().0;
                    }
                    if state.stopped {
                        break;
                    }
                    let packet = state.packet.clone().unwrap();
                    let fresh = std::mem::take(&mut state.fresh);
                    drop(state);
                    if let Err(error) = send(&packet) {
                        lock.lock().unwrap().error = Some(error.to_string());
                        break;
                    }
                    last_sent = Instant::now();
                    if fresh {
                        last_fresh = last_sent;
                    }
                }
            })?;
        Ok(Self {
            state,
            worker: Some(worker),
        })
    }

    pub fn send(&mut self, packet: &[u8]) -> Result<()> {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        if let Some(error) = &state.error {
            return Err(anyhow!("Hue transport failed: {error}"));
        }
        state.packet = Some(packet.to_vec());
        state.fresh = true;
        wake.notify_one();
        Ok(())
    }
}

impl Drop for RepeatingHueStream {
    fn drop(&mut self) {
        let (lock, wake) = &*self.state;
        lock.lock().unwrap().stopped = true;
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_packets_use_the_short_gap_and_repeats_the_keepalive_interval() {
        let t = Instant::now();
        assert_eq!(send_due(t, t, true), t + MIN_SEND_GAP);
        assert_eq!(send_due(t, t, false), t + REPEAT_INTERVAL);
        // A keep-alive repeat just sent does not hold back new colours.
        let fresh_earlier = t - Duration::from_millis(30);
        assert_eq!(
            send_due(t, fresh_earlier, true),
            fresh_earlier + MIN_SEND_GAP
        );
        assert!(MIN_SEND_GAP < REPEAT_INTERVAL);
        // Never faster than ~60 Hz, so the bridge is not flooded.
        assert!(MIN_SEND_GAP >= Duration::from_micros(16_000));
    }
    use std::sync::mpsc;

    #[test]
    fn repeats_latest_packet_without_resampling_and_stops_on_drop() {
        let (sent, received) = mpsc::channel();
        let mut output = RepeatingHueStream::with_sender(move |packet| {
            sent.send(packet.to_vec())?;
            Ok(())
        })
        .unwrap();
        output.send(&[1]).unwrap();
        for _ in 0..2 {
            assert_eq!(received.recv_timeout(Duration::from_secs(1)).unwrap(), [1]);
        }
        output.send(&[2]).unwrap();
        while received.recv_timeout(Duration::from_secs(1)).unwrap() != [2] {}
        drop(output);
        while received.try_recv().is_ok() {}
        assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
        let mut failed =
            RepeatingHueStream::with_sender(|_| anyhow::bail!("simulated failure")).unwrap();
        failed.send(&[1]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while failed.send(&[1]).is_ok() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
    }
}
