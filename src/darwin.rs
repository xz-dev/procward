// SPDX-License-Identifier: BSD-3-Clause
use crate::policy::{Phase, next_signals};
use libproc::{
    bsd_info::BSDInfo,
    proc_pid::pidinfo,
    processes::{ProcFilter, pids_by_type},
};
use rustix::{
    io::Errno,
    process::{self, Pid},
};
use std::{
    collections::{HashMap, HashSet},
    io,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    pid: u32,
    seconds: u64,
    micros: u64,
    uid: u32,
    real_uid: u32,
    saved_uid: u32,
}
impl Identity {
    fn from_info(info: &BSDInfo) -> Self {
        Self {
            pid: info.pbi_pid,
            seconds: info.pbi_start_tvsec,
            micros: info.pbi_start_tvusec,
            uid: info.pbi_uid,
            real_uid: info.pbi_ruid,
            saved_uid: info.pbi_svuid,
        }
    }
}
struct Target {
    identity: Identity,
    term_sent: bool,
}
pub struct Inventory(HashMap<Pid, Target>, HashSet<Pid>);

fn list_children(parent: u32) -> io::Result<Vec<u32>> {
    // libproc 0.14.11 checks errno on a valid empty result without clearing stale errno first.
    errno::set_errno(errno::Errno(0));
    pids_by_type(ProcFilter::ByParentProcess { ppid: parent })
}

fn query(pid: Pid) -> io::Result<Option<BSDInfo>> {
    match pidinfo::<BSDInfo>(pid.as_raw_nonzero().get(), 0) {
        Ok(info) if info.pbi_pid == pid.as_raw_nonzero().get() as u32 => Ok(Some(info)),
        Ok(_) => Err(io::Error::other(
            "libproc returned a mismatched process PID",
        )),
        Err(message) => {
            let query_errno = errno::errno().0;
            match process::test_kill_process(pid) {
                Err(Errno::SRCH) => Ok(None),
                Ok(()) if query_errno == Errno::SRCH.raw_os_error() => Err(io::Error::new(io::ErrorKind::WouldBlock, format!("process {pid} still exists but libproc has no identity yet"))),
                _ => Err(io::Error::other(format!("libproc identity query for {pid} failed: {message}"))),
            }
        },
    }
}

fn signal_if_current(
    expected: Identity,
    current: io::Result<Option<Identity>>,
    send: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    if current? != Some(expected) {
        return Ok(false);
    }
    send()?;
    Ok(true)
}

impl Inventory {
    pub fn new() -> Self {
        Self(HashMap::new(), HashSet::new())
    }
    pub fn refresh(&mut self) -> io::Result<()> {
        let root = process::getpid();
        // Unknown observations remain completion blockers, never signalling authority.
        let mut absent = Vec::new();
        for pid in &self.1 {
            match query(*pid) {
                Ok(None) => absent.push(*pid),
                Ok(Some(_)) => {},
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {},
                Err(error) => return Err(error),
            }
        }
        for pid in absent { self.1.remove(&pid); }
        let mut pending: Vec<_> = self.0.keys().copied().chain([root]).collect();
        let mut visited = HashSet::new();
        while let Some(parent) = pending.pop() {
            if !visited.insert(parent) {
                continue;
            }
            let before = match query(parent) {
                Ok(Some(info)) => info,
                Ok(None) => { self.0.remove(&parent); continue; },
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            let before = Identity::from_info(&before);
            if parent != root
                && self
                    .0
                    .get(&parent)
                    .is_none_or(|target| target.identity != before)
            {
                self.0.remove(&parent);
                continue;
            }
            let mut observed = Vec::new();
            for raw in list_children(before.pid)? {
                let pid = Pid::from_raw(raw as i32)
                    .ok_or_else(|| io::Error::other("invalid libproc child PID"))?;
                match query(pid) {
                    Ok(Some(info)) if info.pbi_ppid == before.pid => observed.push((pid, Identity::from_info(&info))),
                    Ok(_) => {},
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => { self.1.insert(pid); },,
                    Err(error) => return Err(error),
                }
            }
            let after = match query(parent) {
                Ok(info) => info.as_ref().map(Identity::from_info),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            if after != Some(before) {
                continue;
            }
            for (pid, identity) in observed {
                self.1.remove(&pid);
                let entry = self.0.entry(pid).or_insert(Target {
                    identity,
                    term_sent: false,
                });
                if entry.identity != identity {
                    *entry = Target {
                        identity,
                        term_sent: false,
                    };
                }
                pending.push(pid);
            }
        }
        Ok(())
    }
    pub fn advance(&mut self, phase: Phase) -> io::Result<()> {
        let mut gone = Vec::new();
        for (pid, target) in &mut self.0 {
            let current = match query(*pid) {
                Ok(info) => info.as_ref().map(Identity::from_info),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            if current != Some(target.identity) {
                gone.push(*pid);
                continue;
            }
            for signal in next_signals(phase, &mut target.term_sent) {
                // macOS has an unavoidable final identity-check/use race, not kernel containment.
                let current = match query(*pid) {
                    Ok(info) => info.as_ref().map(Identity::from_info),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error),
                };
                match signal_if_current(target.identity, Ok(current), || {
                    process::kill_process(*pid, *signal).map_err(io::Error::from)
                }) {
                    Ok(true) => {}
                    Ok(false) => {
                        gone.push(*pid);
                        break;
                    }
                    Err(error) if error.raw_os_error() == Some(Errno::SRCH.raw_os_error()) => {
                        gone.push(*pid);
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        for pid in gone {
            self.0.remove(&pid);
        }
        Ok(())
    }
    pub fn forget(&mut self, pid: Pid) {
        self.0.remove(&pid);
        self.1.remove(&pid);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty() && self.1.is_empty()
    }
}

pub fn children() -> io::Result<Vec<Pid>> {
    list_children(std::process::id())?
        .into_iter()
        .map(|pid| {
            Pid::from_raw(pid as i32)
                .ok_or_else(|| io::Error::other("invalid libproc direct child"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    fn poll(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }
    struct Fixture {
        child: Child,
        dir: PathBuf,
        worker: Option<Identity>,
        expected_escape: bool,
    }
    impl Fixture {
        fn new(mode: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("procward-darwin-{}-{mode}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let fixture = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("examples/fixture");
            let child = Command::new(fixture)
                .arg(mode)
                .arg(&dir)
                .stdin(Stdio::null())
                .spawn()
                .unwrap();
            Self {
                child,
                dir,
                worker: None,
                expected_escape: false,
            }
        }
        fn remember(&mut self, name: &str) -> Pid {
            let path = self.dir.join(format!("{name}.pid"));
            poll(|| {
                path.try_exists().unwrap()
                    && fs::read_to_string(&path).unwrap().parse::<i32>().is_ok()
            });
            let pid = Pid::from_raw(fs::read_to_string(path).unwrap().parse().unwrap()).unwrap();
            self.worker = Some(Identity::from_info(&query(pid).unwrap().unwrap()));
            pid
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(identity) = self.worker {
                let pid = Pid::from_raw(identity.pid as i32).unwrap();
                if query(pid).unwrap().as_ref().map(Identity::from_info) == Some(identity) {
                    eprintln!(
                        "HARNESS RESCUE: Darwin known survivor {}, expected unseen-escape demonstration: {}; NOT product cleanup",
                        identity.pid, self.expected_escape
                    );
                    process::kill_process(pid, process::Signal::KILL).unwrap();
                    poll(|| match query(pid) {
                        Ok(None) => true,
                        Ok(Some(_)) => false,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => false,
                        Err(error) => panic!("fixture reaping query failed: {error}"),
                    });
                    assert!(
                        self.expected_escape || thread::panicking(),
                        "observed fixture needed rescue"
                    );
                }
            }
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    #[test]
    fn unknown_observation_blocks_completion_without_signal_authority() {
        let mut inventory = Inventory::new();
        inventory.1.insert(process::getpid());
        inventory.advance(Phase::Grace).unwrap();
        assert!(!inventory.is_empty());
        assert!(query(process::getpid()).unwrap().is_some());
    }
    #[test]
    fn changed_birth_pid_or_credentials_and_unknown_queries_never_authorize_signal() {
        let saved = Identity {
            pid: 42,
            seconds: 100,
            micros: 10,
            uid: 501,
            real_uid: 501,
            saved_uid: 501,
        };
        for current in [
            Identity { pid: 43, ..saved },
            Identity {
                micros: 11,
                ..saved
            },
            Identity {
                seconds: 101,
                ..saved
            },
            Identity { uid: 0, ..saved },
            Identity {
                real_uid: 0,
                ..saved
            },
            Identity {
                saved_uid: 0,
                ..saved
            },
        ] {
            assert!(
                !signal_if_current(saved, Ok(Some(current)), || panic!(
                    "stale observation signalled unrelated work"
                ))
                .unwrap()
            );
        }
        assert!(
            signal_if_current(
                saved,
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
                || panic!("unknown query authorized a signal")
            )
            .is_err()
        );
        let mut sent = false;
        assert!(
            signal_if_current(saved, Ok(Some(saved)), || {
                sent = true;
                Ok(())
            })
            .unwrap()
        );
        assert!(sent);
    }
    #[test]
    fn retained_observation_survives_setsid_and_reparenting_but_unseen_escape_is_explicit() {
        {
            let mut fixture = Fixture::new("observed-tree");
            let worker = fixture.remember("observed-worker");
            let mut inventory = Inventory::new();
            poll(|| {
                inventory.refresh().unwrap();
                inventory.0.contains_key(&worker)
            });
            fs::write(fixture.dir.join("detach"), b"go").unwrap();
            poll(|| fixture.child.try_wait().unwrap().is_some());
            assert_eq!(fixture.child.wait().unwrap().code(), Some(23));
            poll(|| fs::metadata(fixture.dir.join("observed-detached")).is_ok());
            inventory.refresh().unwrap();
            assert!(
                inventory.0.contains_key(&worker),
                "reparenting lost observed identity"
            );
            inventory.advance(Phase::Grace).unwrap();
            poll(|| {
                inventory.refresh().unwrap();
                inventory.is_empty()
            });
        }
        {
            let mut fixture = Fixture::new("fast-escape");
            let worker = fixture.remember("escape-worker");
            poll(|| fixture.dir.join("escaped").try_exists().unwrap());
            fs::write(fixture.dir.join("exit-escape"), b"go").unwrap();
            poll(|| fixture.child.try_wait().unwrap().is_some());
            fixture.child.wait().unwrap();
            let mut inventory = Inventory::new();
            inventory.refresh().unwrap();
            assert!(!inventory.0.contains_key(&worker));
            assert!(
                query(worker).unwrap().is_some(),
                "escape fixture was not alive"
            );
            fixture.expected_escape = true;
            eprintln!(
                "EXPECTED MACOS LIMITATION: setsid/reparent before first observation escapes best-effort inventory; rescue follows separately"
            );
        }
    }
}
