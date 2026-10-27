// SPDX-License-Identifier: BSD-3-Clause
use crate::{
    config::Config,
    inventory::Inventory,
    policy::{Policy, RECORD_LEN},
};
use rustix::{
    fd::{AsRawFd, FromRawFd, OwnedFd},
    io::{self as rio, Errno, FdFlags},
    process::{self, Pid, Signal},
};
use signal_hook::{
    consts::{SIGCHLD, SIGHUP, SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    ffi::{OsStr, OsString},
    io,
    os::unix::process::{CommandExt, ExitStatusExt},
    process::Command,
    thread,
    time::Duration,
};

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    #[cfg(not(target_os = "macos"))]
    return Ok(rustix::pipe::pipe_with(
        rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
    )?);
    #[cfg(target_os = "macos")]
    {
        let pair = rustix::pipe::pipe()?;
        for fd in [&pair.0, &pair.1] {
            rio::fcntl_setfd(fd, FdFlags::CLOEXEC)?;
            rustix::fs::fcntl_setfl(fd, rustix::fs::OFlags::NONBLOCK)?;
        }
        Ok(pair)
    }
}

struct PolicyReader {
    bytes: [u8; RECORD_LEN],
    filled: usize,
    policy: Option<Policy>,
}
impl PolicyReader {
    fn new() -> Self {
        Self {
            bytes: [0; RECORD_LEN],
            filled: 0,
            policy: None,
        }
    }
    fn refresh(&mut self, fd: &OwnedFd, config: Config) -> io::Result<()> {
        loop {
            let mut extra = [0];
            let target = if self.filled == RECORD_LEN {
                &mut extra[..]
            } else {
                &mut self.bytes[self.filled..]
            };
            match rio::read(fd, target) {
                Ok(0) if self.filled != 0 && self.filled != RECORD_LEN => {
                    return Err(io::Error::other("truncated native cleanup policy"));
                }
                Ok(0) | Err(Errno::AGAIN) => return Ok(()),
                Err(Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
                Ok(_) if self.filled == RECORD_LEN => {
                    return Err(io::Error::other("duplicate native cleanup policy"));
                }
                Ok(count) => {
                    self.filled += count;
                    if self.filled == RECORD_LEN {
                        self.policy = Some(Policy::from_record(&self.bytes, config)?);
                    }
                }
            }
        }
    }
}

pub fn run(command: &OsStr, args: &[OsString], config: Config) -> io::Result<u8> {
    crate::reaper::acquire()?;
    let mut signals = Signals::new([SIGCHLD, SIGHUP, SIGINT, SIGTERM])?;
    let (read, write) = pipe()?;
    let (notice, publish) = pipe()?;
    let expected = process::getpid();
    let raw = read.as_raw_fd();
    let policy_raw = publish.as_raw_fd();
    let mut anchor = Command::new(std::env::current_exe()?);
    anchor
        .arg("__anchor")
        .arg(expected.to_string())
        .arg(raw.to_string())
        .arg(policy_raw.to_string())
        .arg(config.grace.as_millis().to_string())
        .arg(config.fatal_grace.as_millis().to_string())
        .arg(config.deadline.as_millis().to_string())
        .arg(command)
        .args(args)
        .process_group(0);
    // Only async-signal-safe OS calls run between fork and trusted native exec.
    unsafe {
        anchor.pre_exec(move || {
            arm_parent(expected)?;
            for fd in [raw, policy_raw] {
                rio::fcntl_setfd(rustix::fd::BorrowedFd::borrow_raw(fd), FdFlags::empty())?;
            }
            Ok(())
        });
    }
    let mut anchor = anchor.spawn()?;
    drop(read);
    drop(publish);
    rio::write(&write, b"R")?;
    let pid = Pid::from_raw(anchor.id() as i32).unwrap();
    let mut policy = PolicyReader::new();
    #[cfg(target_os = "macos")]
    let mut observed = Inventory::new();
    loop {
        policy.refresh(&notice, config)?;
        #[cfg(target_os = "macos")]
        observed.refresh()?;
        if let Some(status) = anchor.try_wait()? {
            drop(write);
            // A final read cannot race a dead writer; published policy survives anchor loss.
            policy.refresh(&notice, config)?;
            let policy = policy
                .policy
                .unwrap_or_else(|| Policy::new(config, status.signal().is_some()));
            #[cfg(not(target_os = "macos"))]
            let mut observed = Inventory::new();
            crate::reaper::cleanup(&mut observed, None, &mut None, policy)?;
            return Ok(status.code().unwrap_or(125) as u8);
        }
        for signal in signals.pending().filter(|signal| *signal != SIGCHLD) {
            process::kill_process(pid, Signal::from_named_raw(signal).unwrap())?;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn arm_parent(expected: Pid) -> io::Result<()> {
    // SIGCONT is a targeted kernel wake even if this independent role was stopped.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    process::set_parent_process_death_signal(Some(Signal::CONT))?;
    if process::getppid() != Some(expected) {
        return Err(io::Error::other(
            "owner lost before parent-death wake was armed",
        ));
    }
    Ok(())
}

fn inherited_fd(value: &OsStr, access: rustix::fs::OFlags) -> io::Result<OwnedFd> {
    let raw = value
        .to_str()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|fd| *fd > 2)
        .ok_or_else(|| io::Error::other("invalid anchor native endpoint"))?;
    let borrowed = unsafe { rustix::fd::BorrowedFd::borrow_raw(raw) };
    rio::fcntl_setfd(borrowed, FdFlags::CLOEXEC)?;
    let flags = rustix::fs::fcntl_getfl(borrowed)?;
    let stat = rustix::fs::fstat(borrowed)?;
    if flags & rustix::fs::OFlags::ACCMODE != access
        || rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Fifo
    {
        return Err(io::Error::other("invalid native endpoint type/access"));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

pub fn anchor(args: &[OsString]) -> io::Result<u8> {
    let [
        parent,
        lease,
        publish,
        grace,
        fatal,
        deadline,
        command,
        rest @ ..,
    ] = args
    else {
        return Err(io::Error::other("invalid private anchor invocation"));
    };
    let parent = parent
        .to_str()
        .and_then(|s| s.parse().ok())
        .filter(|pid: &i32| *pid > 0)
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::other("invalid anchor parent"))?;
    arm_parent(parent)?;
    let millis = |value: &OsString| {
        value
            .to_str()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| io::Error::other("invalid native cleanup configuration"))
    };
    let config = Config::from_millis(millis(grace)?, millis(fatal)?, millis(deadline)?)
        .map_err(io::Error::other)?;
    if lease.to_str().and_then(|text| text.parse::<i32>().ok())
        == publish.to_str().and_then(|text| text.parse::<i32>().ok())
    {
        return Err(io::Error::other("duplicate native endpoint"));
    }
    let lease = inherited_fd(lease, rustix::fs::OFlags::RDONLY)?;
    let publish = inherited_fd(publish, rustix::fs::OFlags::WRONLY)?;
    loop {
        let mut grant = [0];
        match rio::read(&lease, &mut grant) {
            Ok(1) if grant[0] == b'R' => break,
            Err(Errno::AGAIN | Errno::INTR) => thread::sleep(Duration::from_millis(1)),
            _ => return Err(io::Error::other("owner lost or invalid launch grant")),
        }
    }
    if !alive(&lease)? || process::getppid() != Some(parent) {
        return Err(io::Error::other("owner lost before payload launch"));
    }
    crate::reaper::run_owned(command, rest, &lease, &publish, config)
}

pub fn publish(fd: &OwnedFd, policy: Policy) -> io::Result<()> {
    let bytes = policy.record();
    loop {
        match rio::write(fd, &bytes) {
            Ok(RECORD_LEN) => return Ok(()),
            Err(Errno::INTR) => continue,
            Ok(_) => {
                return Err(io::Error::other(
                    "incomplete native cleanup policy publication",
                ));
            }
            Err(error) => return Err(error.into()),
        }
    }
}
pub fn alive(lease: impl rustix::fd::AsFd) -> io::Result<bool> {
    let mut byte = [0];
    match rio::read(lease, &mut byte) {
        Ok(0) => Ok(false),
        Err(Errno::AGAIN | Errno::INTR) => Ok(true),
        Ok(_) => Err(io::Error::other("unexpected native lease data")),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_transport_preserves_one_record_and_refuses_partial_or_duplicate_records() {
        let config = Config::default();
        let policy = Policy::new(config, false);
        let (read, write) = pipe().unwrap();
        publish(&write, policy).unwrap();
        drop(write);
        let mut reader = PolicyReader::new();
        reader.refresh(&read, config).unwrap();
        let current = crate::policy::now();
        assert_eq!(reader.policy.unwrap().phase(current), policy.phase(current));
        let (read, write) = pipe().unwrap();
        rio::write(&write, b"PWP1").unwrap();
        drop(write);
        assert!(PolicyReader::new().refresh(&read, config).is_err());
        let (read, write) = pipe().unwrap();
        publish(&write, policy).unwrap();
        publish(&write, policy).unwrap();
        drop(write);
        assert!(PolicyReader::new().refresh(&read, config).is_err());
        let (read, write) = pipe().unwrap();
        drop(write);
        let mut reader = PolicyReader::new();
        reader.refresh(&read, config).unwrap();
        assert!(
            reader.policy.is_none(),
            "no publication means fallback must select its first policy"
        );
    }
}
