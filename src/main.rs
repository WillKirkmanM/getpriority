use clap::{ArgGroup, Parser};
use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(version, about = "Query Linux thread, process group, or user niceness")]
#[command(group(ArgGroup::new("selector").args(["pid", "pgrp", "uid"])))]
struct Arguments {
    #[arg(
        long,
        value_name = "ID",
        help = "Query a thread by its identifier; zero selects the calling thread (default)"
    )]
    pid: Option<u32>,

    #[arg(
        long,
        value_name = "ID",
        help = "Query the lowest niceness in a process group; zero selects the current group"
    )]
    pgrp: Option<u32>,

    #[arg(
        long,
        value_name = "ID",
        help = "Query the lowest niceness for a user; zero selects the caller's real user ID"
    )]
    uid: Option<u32>,

    #[arg(long, help = "Print the selector, identifier, and niceness as JSON")]
    json: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selector {
    Pid,
    Pgrp,
    Uid,
}

impl Selector {
    fn label(self) -> &'static str {
        match self {
            Self::Pid => "pid",
            Self::Pgrp => "pgrp",
            Self::Uid => "uid",
        }
    }

    #[cfg(target_os = "linux")]
    fn which(self) -> std::ffi::c_int {
        match self {
            Self::Pid => 0,
            Self::Pgrp => 1,
            Self::Uid => 2,
        }
    }
}

impl Arguments {
    fn selection(&self) -> (Selector, u32) {
        if let Some(id) = self.pgrp {
            (Selector::Pgrp, id)
        } else if let Some(id) = self.uid {
            (Selector::Uid, id)
        } else {
            (Selector::Pid, self.pid.unwrap_or(0))
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn decode_priority(value: i32, errno: i32) -> io::Result<i32> {
    if value == -1 && errno != 0 {
        Err(io::Error::from_raw_os_error(errno))
    } else {
        Ok(value)
    }
}

#[cfg(target_os = "linux")]
fn query(selector: Selector, id: u32) -> io::Result<i32> {
    use std::ffi::{c_int, c_uint};

    unsafe extern "C" {
        fn getpriority(which: c_int, who: c_uint) -> c_int;
        fn __errno_location() -> *mut c_int;
    }

    let (value, errno) = unsafe {
        let errno = __errno_location();
        *errno = 0;
        let value = getpriority(selector.which(), id);
        (value, *errno)
    };
    decode_priority(value, errno)
}

#[cfg(not(target_os = "linux"))]
fn query(_selector: Selector, _id: u32) -> io::Result<i32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported platform: getpriority requires Linux",
    ))
}

fn render(selector: Selector, id: u32, niceness: i32, json: bool) -> String {
    let selector = selector.label();
    if json {
        format!("{{\"selector\":\"{selector}\",\"identifier\":{id},\"niceness\":{niceness}}}")
    } else {
        format!("Selector: {selector}, identifier: {id}, niceness: {niceness}")
    }
}

fn run(arguments: Arguments) -> Result<(), String> {
    let (selector, id) = arguments.selection();
    let niceness = query(selector, id)
        .map_err(|error| format!("cannot query {} {id}: {error}", selector.label()))?;
    writeln!(
        io::stdout().lock(),
        "{}",
        render(selector, id, niceness, arguments.json)
    )
    .map_err(|error| format!("cannot write output: {error}"))
}

fn main() -> ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("getpriority: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_explicit_zero_keep_kernel_selection() {
        let arguments = Arguments::try_parse_from(["getpriority"]).unwrap();
        assert_eq!(arguments.selection(), (Selector::Pid, 0));
        for (flag, selector) in [
            ("--pid", Selector::Pid),
            ("--pgrp", Selector::Pgrp),
            ("--uid", Selector::Uid),
        ] {
            let arguments = Arguments::try_parse_from(["getpriority", flag, "0"]).unwrap();
            assert_eq!(arguments.selection(), (selector, 0));
        }
    }

    #[test]
    fn rejects_malformed_negative_and_overflowing_identifiers() {
        for flag in ["--pid", "--pgrp", "--uid"] {
            for id in ["not-a-number", "-1", "4294967296", "1.5", ""] {
                assert!(Arguments::try_parse_from(["getpriority", flag, id]).is_err());
            }
            let arguments = Arguments::try_parse_from(["getpriority", flag, "4294967295"]).unwrap();
            assert_eq!(arguments.selection().1, u32::MAX);
        }
    }

    #[test]
    fn selectors_are_mutually_exclusive() {
        for (first, second) in [("--pid", "--pgrp"), ("--pid", "--uid"), ("--pgrp", "--uid")] {
            assert!(Arguments::try_parse_from(["getpriority", first, "0", second, "0"]).is_err());
        }
    }

    #[test]
    fn negative_one_is_success_only_when_errno_is_clear() {
        assert_eq!(decode_priority(-1, 0).unwrap(), -1);
        assert_eq!(decode_priority(-1, 3).unwrap_err().raw_os_error(), Some(3));
        assert_eq!(decode_priority(-20, 0).unwrap(), -20);
        assert_eq!(decode_priority(19, 0).unwrap(), 19);
    }

    #[test]
    fn output_labels_include_selection_and_signed_niceness() {
        assert_eq!(
            render(Selector::Pgrp, 0, -1, true),
            "{\"selector\":\"pgrp\",\"identifier\":0,\"niceness\":-1}"
        );
        assert_eq!(
            render(Selector::Uid, 42, 19, false),
            "Selector: uid, identifier: 42, niceness: 19"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn calling_thread_query_returns_valid_niceness() {
        let niceness = query(Selector::Pid, 0).unwrap();
        assert!((-20..=19).contains(&niceness));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn other_platforms_report_unsupported_operation() {
        let error = query(Selector::Pid, 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("requires Linux"));
    }
}
