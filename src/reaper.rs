// SPDX-License-Identifier: BSD-3-Clause
use crate::{
    config::Config,
    inventory::{Inventory, children},
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
    io,
    process::Command,
    thread,
    time::Duration,
};

pub fn acquire() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    process::set_child_subreaper(Some(Pid::INIT))?;
    #[cfg(target_os = "freebsd")]
    {
        process::set_reaper_status(true)?;
        if !process::get_reaper_status(None)?
            .flags
            .contains(process::ReaperStatusFlags::OWNED)
        {
            return Err(io::Error::other("required FreeBSD reaper was not acquired"));
        }
    }
    Ok(())
}

pub fn run_owned(
    command: &OsStr,
    args: &[OsString],
    lease: &rustix::fd::OwnedFd,
    notice: &rustix::fd::OwnedFd,
    config: Config,
) -> io::Result<u8> {
    acquire()?;
    let mut signals = Signals::new([SIGCHLD, SIGHUP, SIGINT, SIGTERM])?;
    children()?;
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
    let mut requested = None;
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
                    if let Some(code) = status
                        .exit_status()
                        .or_else(|| status.terminating_signal().map(|signal| 128 + signal))
                    {
                        result = Some(code as u8);
                        return Ok(());
                    }
                    // Darwin can return a nonterminal/empty notification with WNOHANG.
                    // Stopped/continued work is still alive and must retain ownership.
                    if status.raw_code() != 0
                        && !status.stopped()
                        && !status.continued()
                        && !status.trapped()
                    {
                        return Err(io::Error::other(format!(
                            "unexpected waitid code {}",
                            status.raw_code()
                        )));
                    }
                }
                Ok(None) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
            reap_adopted(primary, &mut inventory)?;
            // Fatal requests select one immutable policy now, even if primary ignores TERM.
            if let Some(signal) = signals.pending().find(|signal| *signal != SIGCHLD) {
                requested = Some(signal);
                return Ok(());
            }
            thread::sleep(Duration::from_millis(5));
        }
    })();
    let policy = Policy::new(config, false);
    let published = crate::owner::publish(notice, policy);
    let forwarded = match requested {
        Some(signal) => child.signal(signal),
        None => Ok(()),
    };
    cleanup(&mut inventory, Some(primary), &mut result, policy)?;
    published?;
    forwarded?;
    observed?;
    result.ok_or_else(|| io::Error::other("primary result missing after cleanup"))
}

fn reap_adopted(primary: Pid, inventory: &mut Inventory) -> io::Result<()> {
    for pid in children()? {
        if pid != primary {
            match process::waitpid(Some(pid), WaitOptions::NOHANG) {
                Ok(Some((_, status)))
                    if status.exit_status().is_some() || status.terminating_signal().is_some() =>
                {
                    inventory.forget(pid)
                }
                Ok(Some(_)) | Ok(None) | Err(Errno::INTR) => {}
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
    policy: Policy,
) -> io::Result<()> {
    loop {
        let phase = policy.phase(crate::policy::now());
        if phase == Phase::Expired {
            return Err(io::Error::other(
                "cleanup deadline expired: owned children remain",
            ));
        }
        inventory.refresh()?;
        inventory.advance(phase)?;
        match process::wait(WaitOptions::NOHANG) {
            Ok(Some((pid, status))) => {
                if let Some(code) = status
                    .exit_status()
                    .or_else(|| status.terminating_signal().map(|signal| 128 + signal))
                {
                    inventory.forget(pid);
                    if Some(pid) == primary && result.is_none() {
                        *result = Some(code as u8);
                    }
                } else if !status.stopped() && !status.continued() {
                    return Err(io::Error::other("unexpected nonterminal child wait status"));
                }
                continue;
            }
            Err(Errno::INTR) => continue,
            Err(Errno::CHILD) => {
                #[cfg(target_os = "macos")]
                if !inventory.is_empty() {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                return Ok(());
            }
            Err(error) => return Err(error.into()),
            Ok(None) => {}
        }
        thread::sleep(Duration::from_millis(5));
    }
}
