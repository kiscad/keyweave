//! One disposable metadata process; never capture, mutate X devices or run rules.
use clap::Parser;
use serde::Serialize;
use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};
#[cfg(target_os = "linux")]
mod native;
#[derive(Parser)]
struct Arguments {
    #[arg(long)]
    display: String,
    #[arg(long)]
    node: PathBuf,
    #[arg(long)]
    major: u32,
    #[arg(long)]
    minor: u32,
}
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Device {
    id: i32,
    attachment: i32,
    kind: &'static str,
    keys: Vec<i32>,
    buttons: u32,
}
#[derive(Serialize)]
struct Report {
    schema_version: u32,
    display: String,
    node: PathBuf,
    major: u32,
    minor: u32,
    devices: Vec<Device>,
}
fn valid(args: &Arguments) -> bool {
    let Some(value) = args.display.strip_prefix(':') else {
        return false;
    };
    let mut parts = value.split('.');
    let number = parts.next().unwrap_or("");
    let screen = parts.next();
    let node = args
        .node
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("event"));
    args.display.len() <= 64
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && number.parse::<u16>().is_ok()
        && screen.is_none_or(|value| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u16>().is_ok()
        })
        && parts.next().is_none()
        && args.node.parent() == Some(std::path::Path::new("/dev/input"))
        && node.is_some_and(|value| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()))
        && args.major == 13
        && args.minor > 0
}
fn run(args: Arguments) -> io::Result<Report> {
    if !valid(&args) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    #[cfg(target_os = "linux")]
    {
        let devices = native::query(&args)?;
        Ok(Report {
            schema_version: 1,
            display: args.display,
            node: args.node,
            major: args.major,
            minor: args.minor,
            devices,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        Err(io::ErrorKind::Unsupported.into())
    }
}
fn main() -> ExitCode {
    let args = Arguments::parse();
    match run(args) {
        Ok(report) => {
            let mut out = io::stdout().lock();
            if serde_json::to_writer(&mut out, &report).is_err() || out.write_all(b"\n").is_err() {
                return ExitCode::from(74);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("X11 metadata probe unavailable: {:?}", error.kind());
            ExitCode::from(if error.kind() == io::ErrorKind::InvalidInput {
                64
            } else {
                78
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_args() -> Arguments {
        Arguments {
            display: ":0.0".into(),
            node: "/dev/input/event42".into(),
            major: 13,
            minor: 106,
        }
    }
    #[test]
    fn local_display_and_exact_event_node_are_required_before_native_calls() {
        assert!(valid(&sample_args()));
        for display in [
            "remote:0", ":", ":0..1", ":+1", ":0.+1", ":65536", ":0/", ":０",
        ] {
            let mut args = sample_args();
            args.display = display.into();
            assert!(!valid(&args));
        }
        for node in [
            "event42",
            "/tmp/event42",
            "/dev/input/eventx",
            "/dev/input/a/../event42",
        ] {
            let mut args = sample_args();
            args.node = node.into();
            assert!(!valid(&args));
        }
    }
    #[test]
    fn wrong_device_number_or_invalid_args_never_enter_native_query() {
        let mut args = sample_args();
        args.major = 1;
        assert_eq!(
            run(args).err().map(|error| error.kind()),
            Some(io::ErrorKind::InvalidInput)
        );
        let mut args = sample_args();
        args.minor = 0;
        assert!(!valid(&args));
    }
    #[test]
    fn report_is_metadata_only_and_does_not_claim_consumer_readiness() {
        let report = Report {
            schema_version: 1,
            display: ":0".into(),
            node: "/dev/input/event42".into(),
            major: 13,
            minor: 106,
            devices: vec![Device {
                id: 10,
                attachment: 2,
                kind: "keyboard",
                keys: vec![38],
                buttons: 0,
            }],
        };
        let value = serde_json::to_value(report).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(value["schema_version"], 1);
        assert!(value.get("ready").is_none());
        assert!(value.get("pressed").is_none());
        assert!(value.get("window_title").is_none());
    }
}
