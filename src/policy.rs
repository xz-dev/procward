// SPDX-License-Identifier: BSD-3-Clause
use rustix::process::Signal;
use std::time::{Duration, Instant};

pub struct Policy {
    start: Instant,
    grace: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Grace,
    Kill,
    Expired,
}

impl Policy {
    pub fn new(start: Instant, saved_fatal_status: bool) -> Self {
        Self {
            start,
            grace: Duration::from_secs(if saved_fatal_status { 3 } else { 6 }),
        }
    }

    pub fn phase(&self, now: Instant) -> Phase {
        let elapsed = now.duration_since(self.start);
        if elapsed >= Duration::from_millis(7500) {
            Phase::Expired
        } else if elapsed >= self.grace {
            Phase::Kill
        } else {
            Phase::Grace
        }
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
    fn eof_first_keeps_six_seconds_and_original_deadline() {
        let start = Instant::now();
        let eof = Policy::new(start, false);
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
        let start = Instant::now();
        let fatal = Policy::new(start, true);
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
}
