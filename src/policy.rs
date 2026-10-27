// SPDX-License-Identifier: BSD-3-Clause
use crate::config::Config;
use rustix::process::Signal;
use std::{io, time::Duration};

pub const RECORD_LEN: usize = 48;
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    start: Duration,
    config: Config,
    fatal_saved: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Grace,
    Kill,
    Expired,
}

pub fn now() -> Duration {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}
impl Policy {
    pub fn new(config: Config, fatal_saved: bool) -> Self {
        Self::at(now(), config, fatal_saved)
    }
    fn at(start: Duration, config: Config, fatal_saved: bool) -> Self {
        Self {
            start,
            config,
            fatal_saved,
        }
    }
    pub fn phase(&self, now: Duration) -> Phase {
        let elapsed = now.saturating_sub(self.start);
        let grace = if self.fatal_saved {
            self.config.fatal_grace
        } else {
            self.config.grace
        };
        if elapsed >= self.config.deadline {
            Phase::Expired
        } else if elapsed >= grace {
            Phase::Kill
        } else {
            Phase::Grace
        }
    }
    pub fn record(self) -> [u8; RECORD_LEN] {
        let mut bytes = [0; RECORD_LEN];
        bytes[..4].copy_from_slice(b"PWP1");
        bytes[4..12].copy_from_slice(&self.start.as_secs().to_le_bytes());
        bytes[12..16].copy_from_slice(&self.start.subsec_nanos().to_le_bytes());
        bytes[16..24].copy_from_slice(&(self.config.grace.as_millis() as u64).to_le_bytes());
        bytes[24..32].copy_from_slice(&(self.config.fatal_grace.as_millis() as u64).to_le_bytes());
        bytes[32..40].copy_from_slice(&(self.config.deadline.as_millis() as u64).to_le_bytes());
        bytes[40] = u8::from(self.fatal_saved);
        bytes
    }
    pub fn from_record(bytes: &[u8; RECORD_LEN], expected: Config) -> io::Result<Self> {
        let u64_at = |start| u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap());
        let nanos = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        if &bytes[..4] != b"PWP1"
            || nanos >= 1_000_000_000
            || bytes[40] > 1
            || bytes[41..].iter().any(|byte| *byte != 0)
        {
            return Err(io::Error::other("invalid native cleanup policy record"));
        }
        let config =
            Config::from_millis(u64_at(16), u64_at(24), u64_at(32)).map_err(io::Error::other)?;
        let start = Duration::new(u64_at(4), nanos);
        if config != expected || start > now() {
            return Err(io::Error::other(
                "native cleanup policy disagrees with launch configuration/clock",
            ));
        }
        Ok(Self::at(start, config, bytes[40] == 1))
    }
}
pub fn next_signals(phase: Phase, term_sent: &mut bool) -> &'static [Signal] {
    match phase {
        Phase::Grace if !*term_sent => {
            *term_sent = true;
            &[Signal::TERM, Signal::CONT]
        }
        Phase::Kill => &[Signal::KILL],
        _ => &[],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eof_first_keeps_six_seconds_and_original_deadline() {
        let start = now();
        let eof = Policy::at(start, Config::default(), false);
        assert_eq!(eof.phase(start + Duration::from_millis(5999)), Phase::Grace);
        assert_eq!(eof.phase(start + Duration::from_secs(6)), Phase::Kill);
        assert_eq!(eof.phase(start + Duration::from_millis(7499)), Phase::Kill);
        assert_eq!(
            eof.phase(start + Duration::from_millis(7500)),
            Phase::Expired
        );
    }
    #[test]
    fn only_already_saved_fatal_status_selects_three_seconds() {
        let start = now();
        let fatal = Policy::at(start, Config::default(), true);
        assert_eq!(
            fatal.phase(start + Duration::from_millis(2999)),
            Phase::Grace
        );
        assert_eq!(fatal.phase(start + Duration::from_secs(3)), Phase::Kill);
        assert_eq!(
            fatal.phase(start + Duration::from_millis(7500)),
            Phase::Expired
        );
    }
    #[test]
    fn term_precedes_one_cont_and_late_adoption_never_gets_rescue_cont() {
        let mut sent = false;
        assert_eq!(
            next_signals(Phase::Grace, &mut sent),
            &[Signal::TERM, Signal::CONT]
        );
        assert!(next_signals(Phase::Grace, &mut sent).is_empty());
        assert_eq!(next_signals(Phase::Kill, &mut false), &[Signal::KILL]);
        assert!(next_signals(Phase::Expired, &mut false).is_empty());
    }
    #[test]
    fn failover_record_preserves_configured_selection_and_original_start() {
        let start = now();
        let config = Config::from_millis(2000, 1000, 2500).unwrap();
        let policy = Policy::at(start, config, false);
        let restored = Policy::from_record(&policy.record(), config).unwrap();
        assert_eq!(
            restored.phase(start + Duration::from_millis(2000)),
            Phase::Kill
        );
        assert_eq!(
            restored.phase(start + Duration::from_millis(2500)),
            Phase::Expired
        );
        let mut broken = policy.record();
        broken[40] = 2;
        assert!(Policy::from_record(&broken, config).is_err());
        assert!(Policy::from_record(&policy.record(), Config::default()).is_err());
    }
}
