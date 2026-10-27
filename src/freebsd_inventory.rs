// SPDX-License-Identifier: BSD-3-Clause
use crate::policy::{Phase, next_signals};
use rustix::{
    io::Errno,
    process::{self, Pid, PidInfoFlags, Signal},
};
use std::{collections::HashMap, io};

pub struct Inventory(HashMap<Pid, bool>);

impl Inventory {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        process::get_reaper_status(None)?;
        for pid in children()? {
            self.0.entry(pid).or_insert(false);
        }
        Ok(())
    }

    pub fn advance(&mut self, phase: Phase) -> io::Result<()> {
        if phase == Phase::Kill {
            // Kernel targets the entire owned subtree, including nested reapers.
            let result = process::reaper_kill(None, Signal::KILL, false, None)?;
            if let Some(pid) = result.first_failed {
                return Err(io::Error::other(format!(
                    "FreeBSD reaper failed to signal {pid}"
                )));
            }
            return Ok(());
        }
        for (pid, sent) in &mut self.0 {
            // These direct children remain unreaped until after this call.
            for signal in next_signals(phase, sent) {
                match process::kill_process(*pid, *signal) {
                    Ok(()) | Err(Errno::SRCH) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    pub fn forget(&mut self, pid: Pid) {
        self.0.remove(&pid);
    }
}

pub fn children() -> io::Result<Vec<Pid>> {
    Ok(process::get_reaper_pids(None)?
        .into_iter()
        .filter(|info| info.flags.contains(PidInfoFlags::CHILD))
        .map(|info| info.pid)
        .collect())
}
