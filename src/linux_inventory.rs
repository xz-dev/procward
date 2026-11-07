// SPDX-License-Identifier: BSD-3-Clause
use rustix::{
    fd::OwnedFd,
    io::Errno,
    process::{self, Pid, PidfdFlags},
};
use std::{collections::HashMap, fs, io};

pub struct Inventory(
    HashMap<Pid, Target>,
    HashMap<Pid, Retained>,
    crate::policy::Progress,
    Option<crate::cgroup::Scope>,
);

// A descendant reached only through validated parent/birth edges; the pidfd is its identity.
struct Retained {
    fd: OwnedFd,
    birth: u64,
    parent: Pid,
    denied: bool,
}

struct Target {
    fd: Option<OwnedFd>,
    identity: crate::policy::SignalIdentity,
    term_sent: bool,
    denied: bool,
}

impl Inventory {
    pub fn new() -> Self {
        Self(
            HashMap::new(),
            HashMap::new(),
            crate::policy::Progress::default(),
            None,
        )
    }

    pub fn own(&mut self, scope: crate::cgroup::Scope) {
        self.3 = Some(scope);
    }

    // An owned workload group must be empty as well as every direct child reaped.
    pub fn settled(&self) -> io::Result<bool> {
        self.3
            .as_ref()
            .map_or(Ok(true), |scope| Ok(!scope.populated()?))
    }

    pub fn progress(&mut self) -> &mut crate::policy::Progress {
        &mut self.2
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        // Unreaped direct children cannot be reused; detached descendants enter here on adoption.
        for pid in children()? {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.0.entry(pid) {
                entry.insert(Target {
                    fd: open_identity(pid)?,
                    // The unreaped child binds this observation until forget/reap.
                    identity: [
                        pid.as_raw_nonzero().get() as u64,
                        birth(pid)?.unwrap_or(0),
                        0,
                        0,
                        0,
                    ],
                    term_sent: false,
                    denied: false,
                });
            }
        }
        Ok(())
    }

    pub fn advance(
        &mut self,
        phase: crate::policy::Phase,
        protected: Option<Pid>,
    ) -> io::Result<()> {
        for (pid, target) in &mut self.0 {
            if Some(*pid) == protected {
                continue;
            }
            for signal in self
                .2
                .signals(phase, target.identity, &mut target.term_sent)?
            {
                let result = match &target.fd {
                    Some(fd) => process::pidfd_send_signal(fd, *signal),
                    None => process::kill_process(*pid, *signal),
                };
                match result {
                    Ok(()) | Err(Errno::SRCH) => {}
                    // Real/saved UIDs no longer match ours (e.g. after sudo): not signalable, still owned.
                    Err(Errno::PERM) => {
                        if !std::mem::replace(&mut target.denied, true) {
                            crate::diagnostic(format!(
                                "Linux denied signals to privileged descendant {pid}; it remains a known survivor"
                            ));
                        }
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if phase == crate::policy::Phase::Kill {
            if let Some(scope) = &self.3 {
                scope.kill()?;
            }
            self.kill_retained(protected)?;
        }
        Ok(())
    }

    // A live ancestor may never exit, so KILL reaches nondirect descendants without adoption.
    fn kill_retained(&mut self, protected: Option<Pid>) -> io::Result<()> {
        self.discover()?;
        // Bounded walk: stale edges after PID reuse cannot form an endless chain.
        let shielded = |mut pid: Pid, retained: &HashMap<Pid, Retained>| {
            for _ in 0..=retained.len() {
                if Some(pid) == protected {
                    return true;
                }
                match retained.get(&pid) {
                    Some(entry) => pid = entry.parent,
                    None => return false,
                }
            }
            protected.is_some()
        };
        let mut gone = Vec::new();
        for pid in self.1.keys().copied().collect::<Vec<_>>() {
            if shielded(pid, &self.1) {
                continue;
            }
            let entry = self.1.get_mut(&pid).unwrap();
            match process::pidfd_send_signal(&entry.fd, process::Signal::KILL) {
                Ok(()) => {}
                Err(Errno::SRCH) => gone.push(pid),
                Err(Errno::PERM) => {
                    if !std::mem::replace(&mut entry.denied, true) && !self.0.contains_key(&pid) {
                        crate::diagnostic(format!(
                            "Linux denied signals to privileged descendant {pid}; it remains a known survivor"
                        ));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        for pid in gone {
            self.1.remove(&pid);
        }
        Ok(())
    }

    pub fn empty_except(&self, pid: Pid) -> io::Result<bool> {
        Ok(self.0.keys().all(|target| *target == pid))
    }

    pub fn freeze(&mut self, paused: bool) -> io::Result<()> {
        self.discover()?;
        let signal = if paused {
            process::Signal::STOP
        } else {
            process::Signal::CONT
        };
        let mut gone = Vec::new();
        for (pid, entry) in &self.1 {
            // Job control cannot stop a descendant the OS forbids this user to signal.
            match process::pidfd_send_signal(&entry.fd, signal) {
                Ok(()) | Err(Errno::PERM) => {}
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
                    Ok(()) | Err(Errno::SRCH) | Err(Errno::PERM) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    // Extends retained handles from this process and still-valid retained parents only.
    fn discover(&mut self) -> io::Result<()> {
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
                    .is_none_or(|entry| entry.birth != before)
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
            for (pid, fd, birth) in observed {
                // A retained handle keeps its identity across reparenting; never replace it.
                if let Some(entry) = self.1.get_mut(&pid) {
                    if entry.birth == birth {
                        entry.parent = parent;
                        pending.push(pid);
                        continue;
                    }
                }
                self.1.insert(
                    pid,
                    Retained {
                        fd,
                        birth,
                        parent,
                        denied: false,
                    },
                );
                pending.push(pid);
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
