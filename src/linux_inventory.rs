// SPDX-License-Identifier: BSD-3-Clause
use rustix::{
    fd::OwnedFd,
    io::Errno,
    process::{self, Pid, PidfdFlags, Signal},
};
use std::{collections::HashMap, fs, io};

pub struct Inventory(HashMap<Pid, Option<OwnedFd>>);

impl Inventory {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        // Unreaped direct children cannot be reused; detached descendants enter here on adoption.
        for pid in children()? {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.0.entry(pid) {
                entry.insert(open_identity(pid)?);
            }
        }
        Ok(())
    }

    pub fn signal(&self, signal: Signal) -> io::Result<()> {
        for (pid, fd) in &self.0 {
            let result = match fd {
                Some(fd) => process::pidfd_send_signal(fd, signal),
                None => process::kill_process(*pid, signal),
            };
            match result {
                Ok(()) | Err(Errno::SRCH) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub fn forget(&mut self, pid: Pid) {
        self.0.remove(&pid);
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
            eprintln!(
                "procward: pidfd unavailable; nondirect freezing unsupported, direct-child reaper cleanup remains active"
            );
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
