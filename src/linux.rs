// SPDX-License-Identifier: BSD-3-Clause
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

pub fn run(command: &OsStr, args: &[OsString]) -> io::Result<u8> {
    // rustix 1.1.6 represents this boolean prctl argument as Option<Pid>: Some(1) enables it.
    process::set_child_subreaper(Some(Pid::INIT))?;
    let mut signals = Signals::new([SIGCHLD, SIGHUP, SIGINT, SIGTERM])?;
    fs::read_to_string("/proc/thread-self/children")?;
    let mut command = Command::new(command);
    command.args(args);
    let child = CommandWrap::from(command)
        .wrap(ProcessGroup::leader())
        .spawn()?;
    let primary = Pid::from_raw(child.id() as i32).expect("spawn returned a positive PID");
    let result = observe_primary(child.as_ref(), primary, &mut signals);
    let cleanup = cleanup(child.as_ref());
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
) -> io::Result<WaitIdStatus> {
    loop {
        match process::waitid(
            WaitId::Pid(primary),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            Ok(Some(status)) => return Ok(status),
            Ok(None) | Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
        reap_adopted(primary)?;
        for signal in signals.pending().filter(|signal| *signal != SIGCHLD) {
            child.signal(signal)?;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn reap_adopted(primary: Pid) -> io::Result<()> {
    // rustix waitid has no returned-PID accessor; enumerate only our direct children.
    for raw in fs::read_to_string("/proc/thread-self/children")?.split_whitespace() {
        let pid = raw
            .parse()
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| io::Error::other("invalid child PID from procfs"))?;
        if pid != primary {
            match process::waitpid(Some(pid), WaitOptions::NOHANG) {
                Ok(_) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn cleanup(child: &dyn ChildWrapper) -> io::Result<()> {
    // Keep the primary unreaped until the final group signal, so its PGID cannot be reused.
    // shortcut: group-only immediate KILL; add detached identities and graceful policy in section 3.
    let signalled = match child.signal(signal_hook::consts::SIGKILL) {
        Err(error) if error.raw_os_error() == Some(Errno::SRCH.raw_os_error()) => Ok(()),
        result => result,
    };
    let deadline = Instant::now() + Duration::from_millis(7500);
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "cleanup deadline expired: children remain (detached-tree cleanup is not implemented)",
            ));
        }
        match process::wait(WaitOptions::NOHANG) {
            Ok(Some(_)) | Err(Errno::INTR) => continue,
            Err(Errno::CHILD) => return signalled,
            Err(error) => return Err(error.into()),
            Ok(None) => {}
        }
        thread::sleep(Duration::from_millis(5));
    }
}
