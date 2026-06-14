//! `ma-time-filter` — 2D Kalman filter for Sendspin clock offset/drift.
//!
//! Sendspin clients continuously send `client/time` messages; the server
//! replies with `server/time` containing four timestamps. The client uses
//! those to feed this filter, which tracks both:
//!
//! * `offset_us`: the static offset between server and client clocks
//!   (server_clock ≈ client_clock + offset).
//! * `drift_ppm`: the rate of clock drift (parts per million) between the two
//!   clocks.
//!
//! The filter's job is to convert server-clock timestamps (used to schedule
//! audio playback) into local-clock timestamps. The math is a discrete
//! first-order Kalman filter operating in the 2D state space
//! `(offset, drift)`. It is intentionally tiny — no external deps beyond
//! `serde` and `thiserror`.
//!
//! Reference: <https://github.com/Sendspin/time-filter> (C++ reference impl)
//! and `aiosendspin.server.time_filter` (Python reference).

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// 2D Kalman time filter, state = (offset_us, drift_ppm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeFilter {
    /// Mean estimate of the offset (µs). Server_clock ≈ client_clock + offset.
    pub offset_us: f64,
    /// Mean estimate of the drift (parts per million).
    pub drift_ppm: f64,
    /// 2x2 covariance matrix, row-major: `[var_off, cov_off_drift; cov_off_drift, var_drift]`.
    covariance: [[f64; 2]; 2],
    /// Configurable process noise on the offset. Tune higher to make the
    /// filter trust new measurements more aggressively.
    pub process_noise_offset: f64,
    /// Configurable process noise on the drift.
    pub process_noise_drift: f64,
    /// Configurable measurement noise applied to incoming RTT samples.
    pub measurement_noise: f64,
    /// Monotonic time of the last update (µs) — used to project the state
    /// forward on subsequent updates.
    last_update_us: Option<i64>,
}

impl Default for TimeFilter {
    fn default() -> Self {
        // Defaults chosen to match the Sendspin reference implementation:
        // trust the offset estimate but let the drift term accumulate
        // variance over time so the cross-covariance can grow.
        Self {
            offset_us: 0.0,
            drift_ppm: 0.0,
            covariance: [[1.0e-3, 0.0], [0.0, 1.0e-2]],
            process_noise_offset: 1.0e-9,
            process_noise_drift: 1.0e-9,
            measurement_noise: 1.0e-6,
            last_update_us: None,
        }
    }
}

/// Inputs to one `update` step. All values are in microseconds on a
/// monotonic clock, except `rtt_us` which is computed as
/// `client_received - client_transmitted` (i.e. the round-trip time the
/// client observes).
#[derive(Debug, Clone, Copy)]
pub struct TimeUpdate {
    /// Local monotonic time when the client transmitted the `client/time` msg.
    pub client_transmitted_us: i64,
    /// Server's monotonic time at receive.
    pub server_received_us: i64,
    /// Server's monotonic time at transmit of the response.
    pub server_transmitted_us: i64,
    /// Local monotonic time when the client received the `server/time` reply.
    pub client_received_us: i64,
}

impl TimeUpdate {
    /// Round-trip time observed by the client (microseconds).
    pub fn rtt_us(&self) -> i64 {
        self.client_received_us - self.client_transmitted_us
    }

    /// Server-side processing delay (microseconds).
    pub fn server_delay_us(&self) -> i64 {
        self.server_transmitted_us - self.server_received_us
    }
}

impl TimeFilter {
    /// Create a new filter with the default process / measurement noise.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset the filter to its initial state.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed a new 4-timestamp observation. The local clock `t_local_us` is
    /// used to project the state forward between updates.
    pub fn update(&mut self, t_local_us: i64, u: TimeUpdate) {
        // Predict: project state forward by elapsed time.
        if let Some(last) = self.last_update_us {
            let dt = (t_local_us - last) as f64;
            if dt > 0.0 {
                // offset += drift * dt   (drift in ppm, dt in µs, so ×1e-6)
                self.offset_us += self.drift_ppm * 1.0e-6 * dt;
                // Covariance update: P = F P F^T + Q, with F = [[1, dt*1e-6], [0, 1]].
                // P[0][0] += 2*dt*1e-6*P[0][1] + (dt*1e-6)^2*P[1][1] + Q_off*dt
                let dt_ppm = dt * 1.0e-6;
                self.covariance[0][0] += self.process_noise_offset * dt
                    + 2.0 * dt_ppm * self.covariance[0][1]
                    + dt_ppm * dt_ppm * self.covariance[1][1];
                self.covariance[0][1] += dt_ppm * self.covariance[1][1];
                self.covariance[1][0] += dt_ppm * self.covariance[1][1];
                self.covariance[1][1] += self.process_noise_drift * dt;
            }
        }
        self.last_update_us = Some(t_local_us);

        // Measurement: assume symmetry of the RTT — server time is offset by
        // (client_received - rtt/2). This is the standard NTP-style estimate.
        let rtt = u.rtt_us() as f64;
        let half_rtt = rtt * 0.5;
        let measured_offset = u.server_received_us as f64
            - u.client_transmitted_us as f64
            - half_rtt
            - u.server_delay_us() as f64 * 0.5;

        // Residual = measurement - predicted.
        let residual = measured_offset - self.offset_us;

        // Innovation covariance = H P H^T + R, where H = [1, 0].
        let s = self.covariance[0][0] + self.measurement_noise;
        if s.abs() < f64::EPSILON {
            return;
        }

        // Kalman gain K = P H^T / s.
        let k0 = self.covariance[0][0] / s;
        let k1 = self.covariance[1][0] / s;

        // State update.
        self.offset_us += k0 * residual;
        self.drift_ppm += k1 * residual;

        // Covariance update: P = (I - K H) P.
        let p00 = self.covariance[0][0] * (1.0 - k0);
        let p01 = self.covariance[0][1] * (1.0 - k0);
        let p10 = self.covariance[1][0] - k1 * self.covariance[0][0];
        let p11 = self.covariance[1][1] - k1 * self.covariance[0][1];
        self.covariance = [[p00, p01], [p10, p11]];
    }

    /// Convert a server-clock timestamp to a local-clock timestamp using the
    /// current filter state, evaluated at the local time `t_local_us`.
    pub fn compute_client_time(&self, t_local_us: i64, server_time_us: i64) -> i64 {
        // Predicted offset at t_local_us = current offset + drift * dt
        let dt = match self.last_update_us {
            Some(last) => (t_local_us - last) as f64,
            None => 0.0,
        };
        let predicted_offset = self.offset_us + self.drift_ppm * 1.0e-6 * dt;
        // server = client + offset  ⇒  client = server - offset
        (server_time_us as f64 - predicted_offset).round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> TimeFilter {
        TimeFilter::new()
    }

    #[test]
    fn first_update_estimates_offset() {
        let mut f = t0();
        // Pretend the server clock is exactly 1 ms ahead of the client, and
        // the round-trip is small (100 µs) so the offset estimate is sharp.
        let u = TimeUpdate {
            client_transmitted_us: 1_000_000,
            server_received_us: 1_001_000,
            server_transmitted_us: 1_001_050,
            client_received_us: 1_000_100,
        };
        // rtt = 100, half_rtt = 50, server_delay = 50, server_delay/2 = 25
        // measured_offset = 1_001_000 - 1_000_000 - 50 - 25 = 925 us
        f.update(1_000_100, u);
        // Filter estimate should converge near 925 us.
        assert!(
            (f.offset_us - 925.0).abs() < 1.0,
            "offset_us was {}",
            f.offset_us
        );
    }

    #[test]
    fn compute_client_time_inverts() {
        let mut f = t0();
        f.update(
            2_000_000,
            TimeUpdate {
                client_transmitted_us: 1_000_000,
                server_received_us: 1_001_000,
                server_transmitted_us: 1_001_001,
                client_received_us: 1_001_002,
            },
        );
        // At the same local time, server = client + offset.
        let local = f.compute_client_time(2_000_000, 1_001_000);
        // local should be close to 1_000_000 (the client_transmitted_us at
        // which offset was estimated).
        let error = (local - 1_000_000).abs();
        assert!(error < 1_000, "error too large: {error} µs");
    }

    #[test]
    fn drift_tracks_constant_offset_change() {
        // Feed a stream of measurements with a constant rate of offset
        // change (simulating a slow drift). The filter should track it.
        let mut f = t0();
        let start = 1_000_000i64;
        for i in 0..2000 {
            let t = start + i * 100_000; // 100 ms apart
                                         // Simulate a drift of 1000 ppm: each step the offset grows by
                                         // 100 µs (100 ms × 1000 ppm × 1e-6 = 100 us per 100 ms).
            let offset_growth = (i as f64) * 100.0; // 100 µs per 100 ms step
            let server_received = t + 925 + offset_growth as i64;
            let server_transmitted = server_received + 25;
            // Keep RTT = 100 µs, so client_received = client_transmitted + 100.
            let client_received = t + 100;
            f.update(
                client_received,
                TimeUpdate {
                    client_transmitted_us: t,
                    server_received_us: server_received,
                    server_transmitted_us: server_transmitted,
                    client_received_us: client_received,
                },
            );
        }
        // Filter should now report a non-zero drift.
        eprintln!("drift={}", f.drift_ppm);
        assert!(
            f.drift_ppm.abs() > 100.0,
            "expected drift >100 ppm, got {}",
            f.drift_ppm
        );
    }

    #[test]
    fn reset_returns_to_default() {
        let mut f = t0();
        f.update(
            1_000_000,
            TimeUpdate {
                client_transmitted_us: 0,
                server_received_us: 1_000_000,
                server_transmitted_us: 1_000_001,
                client_received_us: 1_000_002,
            },
        );
        f.reset();
        assert_eq!(f, TimeFilter::default());
    }
}
