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
    let mut terminal =
        if std::env::var_os("PROCWARD_PRIVATE_TASK").as_deref() == Some(OsStr::new("1")) {
            None
        } else {
            crate::terminal::Terminal::open()?
        };
    let mut signals = Signals::new([
        SIGCHLD,
        SIGHUP,
        SIGINT,
        SIGTERM,
        signal_hook::consts::SIGTTIN,
        signal_hook::consts::SIGTTOU,
        signal_hook::consts::SIGTSTP,
        signal_hook::consts::SIGCONT,
    ])?;
    children()?;
    let mut handoff = crate::contract::Handoff::new()?;
    let mut payload = if let Some(terminal) = &mut terminal {
        terminal.command(command, args)?
    } else {
        let mut payload = Command::new(command);
        payload.args(args);
        payload
    };
    crate::contract::clear(&mut payload);
    payload.env_remove("PROCWARD_PRIVATE_TASK");
    payload.envs(handoff.environment()?);
    if !crate::owner::alive(lease)? {
        return Err(io::Error::other(
            "owner lost before payload launch permission",
        ));
    }
    // No pre_exec on the untrusted command: libc execvp adds an ENOEXEC shell fallback.
    let child = if terminal.is_some() {
        CommandWrap::from(payload).spawn()?
    } else {
        CommandWrap::from(payload)
            .wrap(ProcessGroup::leader())
            .spawn()?
    };
    let primary = Pid::from_raw(child.id() as i32).expect("spawn returned a positive PID");
    let ready = handoff.ready(child.id());
    let mut inventory = Inventory::new();
    let mut result = None;
    let mut requested = None;
    let mut owner_lost = false;
    let mut paused = false;
    let mut waiting_resume = false;
    let observed = (|| -> io::Result<()> {
        loop {
            if !paused {
                if let Some(terminal) = &mut terminal {
                    terminal.pump()?;
                }
            }
            inventory.refresh()?;
            let (alive, acknowledged) = crate::owner::lease_state(lease)?;
            if acknowledged {
                waiting_resume = false;
            }
            if !alive {
                owner_lost = true;
                return Ok(());
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
                        let _ = rustix::io::write(notice, &[b'X', code as u8]);
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
            let events: Vec<_> = signals.pending().collect();
            if let Some(signal) = events
                .iter()
                .copied()
                .find(|signal| matches!(*signal, SIGHUP | SIGINT | SIGTERM))
            {
                requested = Some(signal);
                return Ok(());
            }
            if let Some(terminal) = &mut terminal {
                let stop =
                    terminal.pause_requested() || events.contains(&signal_hook::consts::SIGTSTP);
                let resume = events.contains(&signal_hook::consts::SIGCONT);
                if resume && paused {
                    inventory.freeze(false)?;
                    paused = false;
                    waiting_resume = true;
                    rustix::io::write(notice, b"R")?;
                }
                if stop && !resume && !paused {
                    inventory.freeze(true)?;
                    terminal.restore()?;
                    paused = true;
                    waiting_resume = false;
                    if rustix::io::write(notice, b"S")? != 1 {
                        return Err(io::Error::other("incomplete native pause acknowledgement"));
                    }
                }
                if paused {
                    inventory.freeze(true)?;
                } else if waiting_resume {
                    // Own live group anchors identity; repeat until outer acknowledges its STOP returned.
                    process::kill_process_group(process::getpgrp(), process::Signal::CONT)?;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    })();
    let policy = Policy::new(config, false);
    let published = crate::owner::publish(notice, policy);
    let notified = if owner_lost || requested.is_some() || observed.is_err() {
        handoff.owner_lost()
    } else {
        Ok(())
    };
    let forwarded = match requested {
        Some(signal) => child.signal(signal),
        None => Ok(()),
    };
    let fatal = observed.is_err() || requested.is_some() || owner_lost;
    let result_sent = result.is_some();
    cleanup(&mut inventory, Some(primary), &mut result, policy)?;
    if !result_sent {
        if let Some(code) = result {
            let _ = rustix::io::write(notice, &[b'X', code]);
        }
    }
    if let Some(terminal) = &mut terminal {
        if !fatal {
            terminal.finish_output();
            while !terminal.drained() {
                if !crate::owner::alive(lease)?
                    || signals
                        .pending()
                        .any(|s| matches!(s, SIGHUP | SIGINT | SIGTERM))
                {
                    break;
                }
                terminal.drain()?;
                thread::sleep(Duration::from_millis(2));
            }
        }
        terminal.restore()?;
    }
    if let Err(error) = published {
        if !owner_lost || error.raw_os_error() != Some(Errno::PIPE.raw_os_error()) {
            return Err(error);
        }
    }
    ready?;
    notified?;
    forwarded?;
    observed?;
    if owner_lost {
        return Ok(125);
    }
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
