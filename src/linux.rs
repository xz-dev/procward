// SPDX-License-Identifier: BSD-3-Clause
use crate::{
    linux_inventory::{Inventory, children},
    policy::{Phase, Policy},
};
use process_wrap::std::{CommandWrap, ProcessGroup};
use rustix::{
    io::Errno,
    process::{self, Pid, WaitId, WaitIdOptions, WaitOptions},
};
use signal_hook::{
    consts::{SIGCHLD, SIGHUP, SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    ffi::{OsStr, OsString},
    fs, io,
    process::Command,
    thread,
    time::{Duration, Instant},
};

pub fn run_owned(
    command: &OsStr,
    args: &[OsString],
    lease: &rustix::fd::OwnedFd,
) -> io::Result<u8> {
    process::set_child_subreaper(Some(Pid::INIT))?;
    let mut signals = Signals::new([SIGCHLD, SIGHUP, SIGINT, SIGTERM])?;
    fs::read_to_string("/proc/thread-self/children")?;
    let mut command = Command::new(command);
    command.args(args);
    if !crate::owner::alive(lease)? {
        return Err(io::Error::other(
            "owner lost before payload launch permission",
        ));
    }
    // No pre_exec on the untrusted command: libc execvp adds an ENOEXEC shell fallback.
    let child = CommandWrap::from(command)
        .wrap(ProcessGroup::leader())
        .spawn()?;
    let primary = Pid::from_raw(child.id() as i32).expect("spawn returned a positive PID");
    let mut inventory = Inventory::new();
    let mut result = None;
    let observed = (|| -> io::Result<()> {
        loop {
            inventory.refresh()?;
            if !crate::owner::alive(lease)? {
                return Err(io::Error::other("owner lease ended"));
            }
            match process::waitid(
                WaitId::Pid(primary),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ) {
                Ok(Some(status)) => {
                    result = Some(
                        status
                            .exit_status()
                            .or_else(|| status.terminating_signal().map(|s| 128 + s))
                            .unwrap() as u8,
                    );
                    return Ok(());
                }
                Ok(None) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
            reap_adopted(primary, &mut inventory)?;
            // Fatal requests select one immutable policy now, even if primary ignores TERM.
            if let Some(signal) = signals.pending().find(|signal| *signal != SIGCHLD) {
                child.signal(signal)?;
                return Ok(());
            }
            thread::sleep(Duration::from_millis(5));
        }
    })();
    cleanup(&mut inventory, Some(primary), &mut result, false)?;
    observed?;
    result.ok_or_else(|| io::Error::other("primary result missing after cleanup"))
}

fn reap_adopted(primary: Pid, inventory: &mut Inventory) -> io::Result<()> {
    for pid in children()? {
        if pid != primary {
            match process::waitpid(Some(pid), WaitOptions::NOHANG) {
                Ok(Some(_)) => inventory.forget(pid),
                Ok(None) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

pub(crate) fn cleanup(
    inventory: &mut Inventory,
    primary: Option<Pid>,
    result: &mut Option<u8>,
    saved_fatal_status: bool,
) -> io::Result<()> {
    let policy = Policy::new(Instant::now(), saved_fatal_status);
    loop {
        let phase = policy.phase(Instant::now());
        if phase == Phase::Expired {
            return Err(io::Error::other(
                "cleanup deadline expired: owned children remain",
            ));
        }
        inventory.refresh()?;
        inventory.advance(phase)?;
        match process::wait(WaitOptions::NOHANG) {
            Ok(Some((pid, status))) => {
                inventory.forget(pid);
                if Some(pid) == primary && result.is_none() {
                    *result = Some(
                        status
                            .exit_status()
                            .or_else(|| status.terminating_signal().map(|s| 128 + s))
                            .unwrap() as u8,
                    );
                }
                continue;
            }
            Err(Errno::INTR) => continue,
            Err(Errno::CHILD) => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(None) => {}
        }
        thread::sleep(Duration::from_millis(5));
    }
}
