//! Private Linux bootstrap arguments; no filesystem lookup or input access.
use crate::{ArgumentError, decimal};
use std::{collections::BTreeSet, ffi::OsString, path::PathBuf};
pub const LINUX_WORKER_ARGUMENT: &str = "--keyweave-linux-worker";
const MAX_INPUTS: usize = keyweave_linux::runtime::MAX_DEVICES;
const MAX_ARGUMENTS: usize = 9 + 2 * MAX_INPUTS;
/// None permanently prevents device opening; Console selects only explicit
/// text VT scope, not graphical desktop readiness.
#[derive(Clone, PartialEq, Eq)]
pub enum SessionProvider {
    None,
    Console { vt: PathBuf },
}
impl std::fmt::Debug for SessionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Console { .. } => "Console",
        })
    }
}
pub struct LinuxArguments {
    pub parent: u32,
    pub run_id: u64,
    pub uinput: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub session_provider: SessionProvider,
}
impl std::fmt::Debug for LinuxArguments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxArguments")
            .field("input_count", &self.inputs.len())
            .field("session_provider", &self.session_provider)
            .finish_non_exhaustive()
    }
}
impl LinuxArguments {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, ArgumentError> {
        let args: Vec<_> = args.into_iter().take(MAX_ARGUMENTS + 1).collect();
        if args.len() < 9
            || args.len() > MAX_ARGUMENTS
            || args.len() % 2 != 1
            || args[0] != LINUX_WORKER_ARGUMENT
            || args[3] != "--uinput"
            || args[5] != "--session-provider"
        {
            return Err(ArgumentError);
        }
        let parent = u32::try_from(decimal(&args[1])?).map_err(|_| ArgumentError)?;
        let run_id = decimal(&args[2])?;
        let (session_provider, start) = if args[6] == "none" {
            (SessionProvider::None, 7)
        } else if args[6] == "console" && args[7] == "--vt" {
            let text = args[8].to_str().ok_or(ArgumentError)?;
            let number = text
                .strip_prefix("/dev/tty")
                .filter(|v| {
                    !v.is_empty() && !v.starts_with('0') && v.bytes().all(|b| b.is_ascii_digit())
                })
                .and_then(|v| v.parse::<u16>().ok())
                .filter(|v| (1..=63).contains(v));
            if number.is_none() {
                return Err(ArgumentError);
            }
            (
                SessionProvider::Console {
                    vt: PathBuf::from(&args[8]),
                },
                9,
            )
        } else {
            return Err(ArgumentError);
        };
        if args.len() <= start {
            return Err(ArgumentError);
        }
        let uinput = PathBuf::from(&args[4]);
        if !uinput.is_absolute() {
            return Err(ArgumentError);
        }
        let mut unique = BTreeSet::new();
        let mut inputs = Vec::new();
        for pair in args[start..].chunks_exact(2) {
            let input = PathBuf::from(&pair[1]);
            if pair[0] != "--input"
                || !input.is_absolute()
                || !unique.insert(input.clone())
                || input == uinput
            {
                return Err(ArgumentError);
            }
            inputs.push(input);
        }
        if inputs.len() > MAX_INPUTS {
            return Err(ArgumentError);
        }
        Ok(Self {
            parent,
            run_id,
            uinput,
            inputs,
            session_provider,
        })
    }
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn args() -> Vec<OsString> {
        vec![
            LINUX_WORKER_ARGUMENT.into(),
            "42".into(),
            "57".into(),
            "--uinput".into(),
            std::env::temp_dir().join("private-output").into(),
            "--session-provider".into(),
            "none".into(),
            "--input".into(),
            std::env::temp_dir().join("private-input").into(),
        ]
    }
    #[test]
    fn fixed_contract_preserves_order_and_does_not_disclose_paths_in_debug() {
        let mut a = args();
        a.extend([
            "--input".into(),
            std::env::temp_dir().join("other-input").into(),
        ]);
        let parsed = LinuxArguments::parse(a).unwrap();
        assert_eq!(parsed.parent, 42);
        assert_eq!(parsed.run_id, 57);
        assert!(
            parsed.inputs[0].ends_with("private-input")
                && parsed.inputs[1].ends_with("other-input")
        );
        assert!(!format!("{parsed:?}").contains("private"));
    }
    #[test]
    fn malformed_fields_duplicates_and_provider_claims_are_rejected() {
        for (index, value) in [
            (1, "0"),
            (1, "01"),
            (1, "4294967296"),
            (2, "+1"),
            (2, "18446744073709551616"),
            (3, "--config"),
            (4, "relative"),
            (5, "--identity"),
            (6, "ready"),
            (7, "--device"),
            (8, "relative"),
        ] {
            let mut a = args();
            a[index] = value.into();
            assert!(LinuxArguments::parse(a).is_err());
        }
        let mut a = args();
        a.extend(["--input".into(), a[8].clone()]);
        assert!(LinuxArguments::parse(a).is_err());
        let mut a = args();
        a[8] = a[4].clone();
        assert!(LinuxArguments::parse(a).is_err());
        assert!(LinuxArguments::parse(args().into_iter().take(8)).is_err());
    }
    #[test]
    fn bounded_iterator_rejects_overflow_without_consuming_unbounded_arguments() {
        let a = args();
        let mut consumed = 0;
        let iter = a
            .into_iter()
            .chain(std::iter::repeat(OsString::from("--input")))
            .inspect(|_| consumed += 1);
        assert!(LinuxArguments::parse(iter).is_err());
        assert_eq!(consumed, MAX_ARGUMENTS + 1);
    }
    #[cfg(unix)]
    #[test]
    fn non_unicode_input_path_is_not_lossily_converted() {
        use std::os::unix::ffi::OsStringExt;
        let mut a = args();
        a[8] = OsString::from_vec(b"/tmp/input-\xff".to_vec());
        assert_eq!(
            LinuxArguments::parse(a.clone()).unwrap().inputs[0].as_os_str(),
            &a[8]
        );
    }
    #[test]
    fn console_requires_explicit_canonical_vt_and_hides_path() {
        let mut a = args();
        a[6] = "console".into();
        a.splice(7..7, ["--vt".into(), "/dev/tty2".into()]);
        let parsed = LinuxArguments::parse(a.clone()).unwrap();
        assert!(
            matches!(parsed.session_provider,SessionProvider::Console {ref vt} if vt==std::path::Path::new("/dev/tty2"))
        );
        assert!(!format!("{parsed:?}").contains("tty2"));
        for bad in [
            "/dev/tty0",
            "/dev/tty01",
            "/dev/tty64",
            "/dev/pts/2",
            "/dev/tty2/",
            "relative",
        ] {
            let mut b = a.clone();
            b[8] = bad.into();
            assert!(LinuxArguments::parse(b).is_err());
        }
        a[7] = "--seat".into();
        assert!(LinuxArguments::parse(a).is_err());
    }
    #[test]
    fn both_modes_keep_64_input_limit_and_none_rejects_console_flags() {
        for console in [false, true] {
            let mut a = args();
            a.truncate(7);
            if console {
                a[6] = "console".into();
                a.extend(["--vt".into(), "/dev/tty63".into()]);
            }
            for n in 0..64 {
                a.extend([
                    "--input".into(),
                    std::env::temp_dir().join(format!("input{n}")).into(),
                ]);
            }
            assert_eq!(LinuxArguments::parse(a.clone()).unwrap().inputs.len(), 64);
            a.extend([
                "--input".into(),
                std::env::temp_dir().join("input64").into(),
            ]);
            assert!(LinuxArguments::parse(a).is_err());
        }
        let mut a = args();
        a.splice(7..7, ["--vt".into(), "/dev/tty1".into()]);
        assert!(LinuxArguments::parse(a).is_err());
    }
}
