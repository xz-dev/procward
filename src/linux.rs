// SPDX-License-Identifier: BSD-3-Clause
use crate::linux_inventory::{Inventory, children};
use process_wrap::std::{ChildWrapper, CommandWrap, ProcessGroup};
use rustix::{
    io::Errno,
    process::{self, Pid, WaitId, WaitIdOptions, WaitIdStatus, WaitOptions},
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
    // rustix 1.1.6 represents this boolean prctl argument as Option<Pid>: Some(1) enables it.
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
    // No pre_exec on the untrusted command: libc execvp would add an ENOEXEC shell fallback.
    let child = CommandWrap::from(command)
        .wrap(ProcessGroup::leader())
        .spawn()?;
    let primary = Pid::from_raw(child.id() as i32).expect("spawn returned a positive PID");
    let mut inventory = Inventory::new();
    let result = observe_primary(child.as_ref(), primary, &mut signals, &mut inventory, lease);
    let cleanup = cleanup(&mut inventory);
    cleanup?;
    let status = result?;
    Ok(status
        .exit_status()
        .or_else(|| status.terminating_signal().map(|signal| 128 + signal))
        .expect("waitid EXITED returned an exit or signal status") as u8)
}

fn observe_primary(
    child: &dyn ChildWrapper,
    primary: Pid,
    signals: &mut Signals,
    inventory: &mut Inventory,
    lease: &rustix::fd::OwnedFd,
) -> io::Result<WaitIdStatus> {
    loop {
        if !crate::owner::alive(lease)? {
            return Err(io::Error::other("owner lease ended"));
        }
        inventory.refresh()?;
        match process::waitid(
            WaitId::Pid(primary),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(Some(status)) => return Ok(status),
            Ok(None) | Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
        reap_adopted(primary, inventory)?;
        for signal in signals.pending().filter(|signal| *signal != SIGCHLD) {
            child.signal(signal)?;
        }
        thread::sleep(Duration::from_millis(5));
    }
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

pub(crate) fn cleanup(inventory: &mut Inventory) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(7500);
    loop {
        inventory.refresh()?;
        inventory.signal(process::Signal::KILL)?;
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "cleanup deadline expired: owned children remain",
            ));
        }
        match process::wait(WaitOptions::NOHANG) {
            Ok(Some((pid, _))) => inventory.forget(pid),
            Err(Errno::INTR) => continue,
            Err(Errno::CHILD) => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(None) => {}
        }
        thread::sleep(Duration::from_millis(5));
    }
}
