// SPDX-License-Identifier: BSD-3-Clause
use crate::policy::{Phase, Progress, SignalIdentity};
use rustix::{
    io::Errno,
    process::{self, Pid, PidInfoFlags, Signal},
};
use std::{collections::HashMap, io};

pub struct Inventory(HashMap<Pid, (bool, SignalIdentity)>, Progress);

impl Inventory {
    pub fn new() -> Self {
        Self(HashMap::new(), Progress::default())
    }

    pub fn progress(&mut self) -> &mut Progress {
        &mut self.1
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        process::get_reaper_status(None)?;
        for pid in children()? {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.0.entry(pid) {
                if let Some(identity) = identity(pid)? {
                    entry.insert((false, identity));
                }
            }
        }
        Ok(())
    }

    pub fn advance(&mut self, phase: Phase, protected: Option<Pid>) -> io::Result<()> {
        if phase == Phase::Kill && protected.is_none() {
            // Kernel targets the entire owned subtree, including nested reapers.
            let result = match process::reaper_kill(None, Signal::KILL, false, None) {
                Ok(result) => result,
                Err(Errno::SRCH) => {
                    let remaining = process::get_reaper_pids(None)?;
                    if remaining.iter().all(|info| {
                        info.flags
                            .intersects(PidInfoFlags::ZOMBIE | PidInfoFlags::EXITING)
                    }) {
                        // No signalable member remains; wait/reaper facts still decide completion.
                        return Ok(());
                    }
                    return Err(Errno::SRCH.into());
                }
                Err(error) => return Err(error.into()),
            };
            if let Some(pid) = result.first_failed {
                return Err(io::Error::other(format!(
                    "FreeBSD reaper failed to signal {pid}"
                )));
            }
            return Ok(());
        }
        for (pid, (sent, identity)) in &mut self.0 {
            if Some(*pid) == protected {
                continue;
            }
            // These direct children remain unreaped until after this call.
            for signal in self.1.signals(phase, *identity, sent)? {
                match process::kill_process(*pid, *signal) {
                    Ok(()) | Err(Errno::SRCH) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    pub fn empty_except(&self, pid: Pid) -> io::Result<bool> {
        Ok(process::get_reaper_pids(None)?
            .iter()
            .all(|info| info.pid == pid))
    }

    pub fn freeze(&mut self, paused: bool) -> io::Result<()> {
        let signal = if paused { Signal::STOP } else { Signal::CONT };
        let result = process::reaper_kill(None, signal, false, None)?;
        if let Some(pid) = result.first_failed {
            return Err(io::Error::other(format!(
                "FreeBSD reaper failed to signal {pid}"
            )));
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

fn identity(pid: Pid) -> io::Result<Option<SignalIdentity>> {
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_PID,
        pid.as_raw_nonzero().get(),
    ];
    let mut info: libc::kinfo_proc = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of_val(&info);
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as _,
            (&mut info as *mut libc::kinfo_proc).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if size == 0 {
        return Ok(None);
    }
    if size != std::mem::size_of_val(&info) || info.ki_pid != pid.as_raw_nonzero().get() {
        return Err(io::Error::other("invalid FreeBSD child identity"));
    }
    Ok(Some([
        info.ki_pid as u64,
        info.ki_start.tv_sec as u64,
        info.ki_start.tv_usec as u64,
        0,
        0,
    ]))
}
