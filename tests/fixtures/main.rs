// SPDX-License-Identifier: BSD-3-Clause
use std::{
    env,
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::unix::ffi::OsStrExt,
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
    let bytes = value.as_bytes();
    out.write_all(&(bytes.len() as u32).to_le_bytes()).unwrap();
    out.write_all(bytes).unwrap();
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
