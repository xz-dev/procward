// SPDX-License-Identifier: BSD-3-Clause
#![cfg(target_os = "macos")]
use libproc::{bsd_info::BSDInfo, proc_pid::pidinfo};
use rustix::{
    io::Errno,
    process::{self, Pid, Signal},
};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn poll(deadline: Instant, mut predicate: impl FnMut() -> bool) {
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "Darwin observation deadline expired"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn facts(pid: u32) -> io::Result<Option<BSDInfo>> {
    match pidinfo::<BSDInfo>(pid as i32, 0) {
        Ok(info) => Ok(Some(info)),
        Err(error) => match process::test_kill_process(Pid::from_raw(pid as i32).unwrap()) {
            Err(Errno::SRCH) => Ok(None),
            _ => Err(io::Error::other(error)),
        },
    }
}
fn same(a: &BSDInfo, b: &BSDInfo) -> bool {
    (
        a.pbi_pid,
        a.pbi_start_tvsec,
        a.pbi_start_tvusec,
        a.pbi_uid,
        a.pbi_ruid,
        a.pbi_svuid,
    ) == (
        b.pbi_pid,
        b.pbi_start_tvsec,
        b.pbi_start_tvusec,
        b.pbi_uid,
        b.pbi_ruid,
        b.pbi_svuid,
    )
}
struct Run {
    child: Child,
    dir: PathBuf,
    observed: Vec<BSDInfo>,
}
impl Run {
    fn start(serial: usize) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "procward-darwin-cli-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        let fixture = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/fixture");
        let child = Command::new(env!("CARGO_BIN_EXE_procward"))
            .args(["run", "--"])
            .arg(fixture)
            .arg("tree")
            .arg(&dir)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut run = Self {
            child,
            dir,
            observed: Vec::new(),
        };
        for mode in ["tree", "branch", "worker"] {
            let path = run.dir.join(format!("{mode}.pid"));
            poll(Instant::now() + Duration::from_secs(20), || {
                path.try_exists().unwrap()
                    && fs::read_to_string(&path).unwrap().parse::<u32>().is_ok()
            });
            run.observed.push(
                facts(fs::read_to_string(path).unwrap().parse().unwrap())
                    .unwrap()
                    .unwrap(),
            );
        }
        run.observed
            .push(facts(run.observed[0].pbi_ppid).unwrap().unwrap());
        run
    }
    fn finish(&mut self) -> std::process::ExitStatus {
        poll(Instant::now() + Duration::from_secs(8), || {
            self.child.try_wait().unwrap().is_some()
        });
        self.child.wait().unwrap()
    }
    fn gone(&self) -> bool {
        self.observed.iter().all(|info| {
            facts(info.pbi_pid)
                .unwrap()
                .is_none_or(|now| !same(info, &now))
        })
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        let mut rescued = false;
        for info in &self.observed {
            if facts(info.pbi_pid)
                .unwrap()
                .is_some_and(|now| same(info, &now))
            {
                process::kill_process(Pid::from_raw(info.pbi_pid as i32).unwrap(), Signal::KILL)
                    .unwrap();
                rescued = true;
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        poll(Instant::now() + Duration::from_secs(3), || self.gone());
        fs::remove_dir_all(&self.dir).unwrap();
        if rescued {
            eprintln!("HARNESS RESCUE: known Darwin survivor; NOT product best-effort sweep");
        }
        assert!(
            !rescued || thread::panicking(),
            "observed fixture needed rescue"
        );
    }
}
#[test]
fn darwin_best_effort_tree_owner_loss_and_observer_loss() {
    {
        let mut run = Run::start(0);
        run.child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
        assert_eq!(run.finish().code(), Some(23));
        poll(Instant::now() + Duration::from_secs(1), || run.gone());
    }
    {
        let mut run = Run::start(1);
        for info in &run.observed[..3] {
            process::kill_process(Pid::from_raw(info.pbi_pid as i32).unwrap(), Signal::STOP)
                .unwrap();
        }
        let fault = Instant::now();
        run.child.kill().unwrap();
        assert!(!run.finish().success());
        poll(fault + Duration::from_secs(8), || run.gone());
    }
    {
        let mut run = Run::start(2);
        let anchor = run.observed.last().unwrap();
        assert!(same(anchor, &facts(anchor.pbi_pid).unwrap().unwrap()));
        let fault = Instant::now();
        process::kill_process(Pid::from_raw(anchor.pbi_pid as i32).unwrap(), Signal::KILL).unwrap();
        assert_eq!(
            run.finish().code(),
            Some(125),
            "observer loss must fail closed"
        );
        poll(fault + Duration::from_secs(8), || run.gone());
    }
}
