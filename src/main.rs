// SPDX-License-Identifier: BSD-3-Clause
use std::{env, ffi::OsString, process::ExitCode};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_inventory;
#[cfg(target_os = "linux")]
mod owner;

const USAGE: &str = "procward 0.1.0\n\nUsage: procward run -- <command> [args...]\n       procward --help\n       procward --version\n\nLinux noninteractive walking skeleton; detached-tree and owner-loss cleanup not yet supported.";

fn main() -> ExitCode {
    match cli(env::args_os().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("procward: {message}");
            ExitCode::from(125)
        }
    }
}

fn cli(args: Vec<OsString>) -> Result<u8, String> {
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{USAGE}");
            Ok(0)
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("procward {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        #[cfg(target_os = "linux")]
        [role, rest @ ..] if role == "__anchor" => {
            owner::anchor(rest).map_err(|error| error.to_string())
        }
        [run, separator, command, rest @ ..]
            if run == "run" && separator == "--" && !command.is_empty() =>
        {
            #[cfg(target_os = "linux")]
            return owner::run(command, rest).map_err(|error| error.to_string());
            #[cfg(not(target_os = "linux"))]
            {
                let _ = rest;
                Err("this development slice supports Linux only".into())
            }
        }
        _ => Err("expected run -- <command> [args...]; see --help".into()),
    }
}
