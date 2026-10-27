// SPDX-License-Identifier: BSD-3-Clause
use std::{
    env,
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    path::Path,
    process::{self, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<_> = env::args_os().collect();
    match args[1].to_str().unwrap() {
        "probe" => {
            let mut out = io::stdout().lock();
            for arg in &args[2..] {
                record(&mut out, arg);
            }
            record(&mut out, env::current_dir().unwrap().as_os_str());
            record(&mut out, &env::var_os("PROCWARD_TEST_VALUE").unwrap());
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).unwrap();
            out.write_all(&input).unwrap();
            out.flush().unwrap();
            eprintln!("fixture-stderr");
            process::exit(23);
        }
        "exit" => process::exit(args[2].to_str().unwrap().parse().unwrap()),
        "touch" => fs::write(&args[2], b"payload ran").unwrap(),
        mode => {
            let dir = Path::new(&args[2]);
            fs::write(dir.join(format!("{mode}.pid")), process::id().to_string()).unwrap();
            match mode {
                #[cfg(target_os = "linux")]
                "cleanup-order" => {
                    let _worker = spawn("order-worker", dir);
                    wait_for(&dir.join("order-ready"));
                    wait_for_input();
                    process::exit(23);
                }
                #[cfg(target_os = "linux")]
                "order-worker" => {
                    use std::sync::{
                        Arc,
                        atomic::{AtomicBool, AtomicUsize, Ordering},
                    };
                    let term = Arc::new(AtomicBool::new(false));
                    signal_hook::flag::register(signal_hook::consts::SIGTERM, term.clone())
                        .unwrap();
                    let cont = Arc::new(AtomicUsize::new(0));
                    let seen = cont.clone();
                    unsafe {
                        signal_hook::low_level::register(signal_hook::consts::SIGCONT, move || {
                            seen.fetch_add(1, Ordering::SeqCst);
                        })
                        .unwrap();
                    }
                    fs::write(dir.join("order-ready"), b"ready").unwrap();
                    rustix::process::kill_process(
                        rustix::process::getpid(),
                        rustix::process::Signal::STOP,
                    )
                    .unwrap();
                    loop {
                        fs::write(
                            dir.join("order-observed"),
                            format!(
                                "{} {}",
                                term.load(Ordering::SeqCst),
                                cont.load(Ordering::SeqCst)
                            ),
                        )
                        .unwrap();
                        thread::park_timeout(Duration::from_millis(5));
                    }
                }
                #[cfg(target_os = "linux")]
                "stubborn" => {
                    signal_hook::flag::register(
                        signal_hook::consts::SIGTERM,
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    )
                    .unwrap();
                    fs::write(dir.join("ready"), b"ready").unwrap();
                    loop {
                        thread::park();
                    }
                }
                #[cfg(target_os = "linux")]
                "detached-tree" => {
                    let _branch = spawn("detached-branch", dir);
                    wait_for(&dir.join("stopped-worker.pid"));
                    wait_for_input();
                    process::exit(23);
                }
                #[cfg(target_os = "linux")]
                "detached-branch" => {
                    rustix::process::setsid().unwrap();
                    spawn("stopped-worker", dir).wait().unwrap();
                }
                #[cfg(target_os = "linux")]
                "stopped-worker" => {
                    rustix::process::kill_process(
                        rustix::process::getpid(),
                        rustix::process::Signal::STOP,
                    )
                    .unwrap();
                    loop {
                        thread::park();
                    }
                }
                "tree" => {
                    let _branch = spawn("branch", dir);
                    wait_for(&dir.join("worker.pid"));
                    wait_for_input();
                    process::exit(23);
                }
                "branch" => {
                    spawn("worker", dir).wait().unwrap();
                }
                "worker" => loop {
                    thread::park();
                },
                "orphan" => {
                    assert_eq!(spawn("orphan-parent", dir).wait().unwrap().code(), Some(17));
                    fs::write(dir.join("ready"), b"ready").unwrap();
                    wait_for_input();
                    process::exit(23);
                }
                "orphan-parent" => {
                    let _orphan = spawn("orphan-child", dir);
                    wait_for(&dir.join("orphan-child.pid"));
                    process::exit(17);
                }
                "orphan-child" => {
                    wait_for(&dir.join("release"));
                    process::exit(19);
                }
                "waiting" => wait_for_input(),
                _ => panic!("unknown fixture mode: {mode}"),
            }
        }
    }
}

fn record(out: &mut impl Write, value: &OsStr) {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().to_vec()
    };
    #[cfg(windows)]
    let bytes: Vec<_> = {
        use std::os::windows::ffi::OsStrExt;
        value.encode_wide().flat_map(u16::to_le_bytes).collect()
    };
    out.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
    out.write_all(&bytes).unwrap();
}

fn spawn(mode: &str, dir: &Path) -> process::Child {
    Command::new(env::current_exe().unwrap())
        .arg(mode)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.try_exists().unwrap() {
        assert!(Instant::now() < deadline, "fixture timed out: {path:?}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_input() {
    let mut byte = [0];
    io::stdin().read_exact(&mut byte).unwrap();
}
