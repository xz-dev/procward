// SPDX-License-Identifier: BSD-3-Clause
use rustix::{
    fd::OwnedFd,
    io::Errno,
    process::{self, Pid, PidfdFlags},
};
use std::{collections::HashMap, fs, io};

pub struct Inventory(HashMap<Pid, Target>, HashMap<Pid, (OwnedFd, u64)>);

struct Target {
    fd: Option<OwnedFd>,
    term_sent: bool,
}

impl Inventory {
    pub fn new() -> Self {
        Self(HashMap::new(), HashMap::new())
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        // Unreaped direct children cannot be reused; detached descendants enter here on adoption.
        for pid in children()? {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.0.entry(pid) {
                entry.insert(Target {
                    fd: open_identity(pid)?,
                    term_sent: false,
                });
            }
        }
        Ok(())
    }

    pub fn advance(&mut self, phase: crate::policy::Phase) -> io::Result<()> {
        for (pid, target) in &mut self.0 {
            for signal in crate::policy::next_signals(phase, &mut target.term_sent) {
                let result = match &target.fd {
                    Some(fd) => process::pidfd_send_signal(fd, *signal),
                    None => process::kill_process(*pid, *signal),
                };
                match result {
                    Ok(()) | Err(Errno::SRCH) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    pub fn freeze(&mut self, paused: bool) -> io::Result<()> {
        let root = process::getpid();
        let mut pending: Vec<_> = self.1.keys().copied().chain([root]).collect();
        let mut visited = std::collections::HashSet::new();
        while let Some(parent) = pending.pop() {
            if !visited.insert(parent) {
                continue;
            }
            let Some(before) = birth(parent)? else {
                continue;
            };
            if parent != root
                && self
                    .1
                    .get(&parent)
                    .is_none_or(|(_, expected)| *expected != before)
            {
                continue;
            }
            let tasks = match fs::read_dir(format!("/proc/{parent}/task")) {
                Ok(tasks) => tasks,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut observed = Vec::new();
            for task in tasks {
                let text = match fs::read_to_string(task?.path().join("children")) {
                    Ok(text) => text,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                for raw in text.split_whitespace() {
                    let pid = raw
                        .parse()
                        .ok()
                        .and_then(Pid::from_raw)
                        .ok_or_else(|| io::Error::other("invalid descendant PID"))?;
                    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
                        Ok(text) => text,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    let actual_parent = stat
                        .rsplit_once(')')
                        .and_then(|(_, fields)| fields.split_whitespace().nth(1))
                        .and_then(|v| v.parse::<i32>().ok())
                        .and_then(Pid::from_raw);
                    if actual_parent != Some(parent) {
                        continue;
                    }
                    let observed_birth = parse_birth(&stat)?;
                    if let Some(fd) = open_identity(pid)? {
                        if birth(pid)? == Some(observed_birth) {
                            observed.push((pid, fd, observed_birth));
                        }
                    }
                }
            }
            // A stable handle plus live birth checks bind each edge to the observed parent.
            if birth(parent)? != Some(before) {
                continue;
            }
            for (pid, fd, identity) in observed {
                self.1.insert(pid, (fd, identity));
                pending.push(pid);
            }
        }
        let signal = if paused {
            process::Signal::STOP
        } else {
            process::Signal::CONT
        };
        let mut gone = Vec::new();
        for (pid, (fd, _)) in &self.1 {
            match process::pidfd_send_signal(fd, signal) {
                Ok(()) => {}
                Err(Errno::SRCH) => gone.push(*pid),
                Err(error) => return Err(error.into()),
            }
        }
        for pid in gone {
            self.1.remove(&pid);
        }
        for (pid, target) in &self.0 {
            if target.fd.is_none() {
                match process::kill_process(*pid, signal) {
                    Ok(()) | Err(Errno::SRCH) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    pub fn forget(&mut self, pid: Pid) {
        self.0.remove(&pid);
        self.1.remove(&pid);
    }
}

pub fn children() -> io::Result<Vec<Pid>> {
    fs::read_to_string("/proc/thread-self/children")?
        .split_whitespace()
        .map(|raw| {
            raw.parse()
                .ok()
                .and_then(Pid::from_raw)
                .ok_or_else(|| io::Error::other("invalid child PID from procfs"))
        })
        .collect()
}

fn birth(pid: Pid) -> io::Result<Option<u64>> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => parse_birth(&text).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn parse_birth(stat: &str) -> io::Result<u64> {
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other("invalid process birth from procfs"))
}

fn open_identity(pid: Pid) -> io::Result<Option<OwnedFd>> {
    let Some(before) = birth(pid)? else {
        return Ok(None);
    };
    let fd = match process::pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(Errno::SRCH) => return Ok(None),
        Err(Errno::NOSYS | Errno::PERM | Errno::INVAL) => {
            static REPORTED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::diagnostic("pidfd unavailable; nondirect freezing unsupported, direct-child reaper cleanup remains active".into());
            }
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    match birth(pid)? {
        Some(after) if before == after => Ok(Some(fd)),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn birth_parser_handles_spaces_and_parentheses_in_comm() {
        let stat = "23 (worker ) (name) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4242 0";
        assert_eq!(parse_birth(stat).unwrap(), 4242);
        assert!(parse_birth("23 (truncated) S 1").is_err());
    }
}
