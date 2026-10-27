// SPDX-License-Identifier: BSD-3-Clause
use std::{env, ffi::OsString, process::ExitCode};
mod config;
#[cfg(target_os = "linux")]
#[path = "linux_inventory.rs"]
mod inventory;
#[cfg(target_os = "freebsd")]
#[path = "freebsd_inventory.rs"]
mod inventory;
#[cfg(target_os = "macos")]
#[path = "darwin.rs"]
mod inventory;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod owner;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod policy;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod reaper;
#[cfg(windows)]
mod windows;

const USAGE: &str = "procward 0.1.0\n\nUsage: procward run [options] -- <command> [args...]\n       procward --help\n       procward --version\n\nOptions: --grace <dur> --fatal-grace <dur> --cleanup-deadline <dur>\nDurations: integer ms or s suffix (for example 750ms, 6s).\n\nNoninteractive development build; interactive Unix job control is not yet supported.";
fn main() -> ExitCode {
    match cli(env::args_os().skip(1).collect()) {
        Ok(code) => {
            #[cfg(windows)]
            std::process::exit(code as i32);
            #[cfg(not(windows))]
            ExitCode::from(code as u8)
        }
        Err((code, message)) => {
            eprintln!("procward: {message}");
            ExitCode::from(code)
        }
    }
}
fn cli(args: Vec<OsString>) -> Result<u32, (u8, String)> {
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            Ok(0)
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("procward {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
        [role, rest @ ..] if role == "__anchor" => owner::anchor(rest)
            .map(u32::from)
            .map_err(|error| (125, error.to_string())),
        [run, options @ ..] if run == "run" => {
            let (config, index) = config::options(options)?;
            let [command, rest @ ..] = &options[index..] else {
                return Err((125, "missing command after --".into()));
            };
            if command.is_empty() {
                return Err((125, "command must not be empty".into()));
            }
            #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
            return owner::run(command, rest, config)
                .map(u32::from)
                .map_err(|error| (125, error.to_string()));
            #[cfg(windows)]
            return windows::run(command, rest, config).map_err(|error| (125, error.to_string()));
            #[cfg(not(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "macos",
                windows
            )))]
            {
                let _ = (rest, config);
                Err((125, "unsupported platform".into()))
            }
        }
        _ => Err((
            125,
            "expected run [options] -- <command> [args...]; see --help".into(),
        )),
    }
}
