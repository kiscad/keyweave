//! Private Linux bootstrap arguments; no filesystem lookup or input access.
use crate::{ArgumentError, decimal};
use keyweave_linux::runtime::ExecutionMode;
use std::{collections::BTreeSet, ffi::OsString, path::PathBuf};
pub const LINUX_WORKER_ARGUMENT: &str = "--keyweave-linux-worker";
const MAX_INPUTS: usize = keyweave_linux::runtime::MAX_DEVICES;
const MAX_ARGUMENTS: usize = 15 + 2 * MAX_INPUTS;
/// None permanently prevents device opening; Console selects only explicit
/// text VT scope, not graphical desktop readiness.
#[derive(Clone, PartialEq, Eq)]
pub enum SessionProvider {
    None,
    Console {
        vt: PathBuf,
    },
    X11 {
        helper: PathBuf,
        axes: keyweave_linux::relative::RelativeAxes,
        buttons: Option<keyweave_linux::keys::PointingButtons>,
    },
}
impl std::fmt::Debug for SessionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Console { .. } => "Console",
            Self::X11 { .. } => "X11",
        })
    }
}
pub struct LinuxArguments {
    pub parent: u32,
    pub run_id: u64,
    pub uinput: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub session_provider: SessionProvider,
    pub execution_mode: ExecutionMode,
}
impl std::fmt::Debug for LinuxArguments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxArguments")
            .field("input_count", &self.inputs.len())
            .field("session_provider", &self.session_provider)
            .field("execution_mode", &self.execution_mode)
            .finish_non_exhaustive()
    }
}
fn list_text(codes: impl Iterator<Item = u16>) -> String {
    codes
        .map(|code| code.to_string())
        .collect::<Vec<_>>()
        .join(",")
}
fn numbers(value: &std::ffi::OsStr) -> Result<Vec<u16>, ArgumentError> {
    let text = value.to_str().ok_or(ArgumentError)?;
    if text.is_empty() || text.len() > 48 {
        return Err(ArgumentError);
    }
    let mut codes = Vec::new();
    for part in text.split(',') {
        if part.is_empty()
            || !part.bytes().all(|v| v.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(ArgumentError);
        }
        let code = part.parse::<u16>().map_err(|_| ArgumentError)?;
        if codes.last().is_some_and(|last| *last >= code) {
            return Err(ArgumentError);
        }
        codes.push(code);
    }
    Ok(codes)
}
impl LinuxArguments {
    pub fn capture_scope(&self) -> crate::linux_preparation::CaptureScope {
        if matches!(self.session_provider, SessionProvider::X11 { .. }) {
            crate::linux_preparation::CaptureScope::StagedRelative
        } else {
            crate::linux_preparation::CaptureScope::Keyboard
        }
    }
    pub fn output_spec(&self) -> std::io::Result<keyweave_linux::output::DeviceSpec> {
        let mut spec = crate::linux_preparation::default_output_spec()?;
        if let SessionProvider::X11 { axes, buttons, .. } = &self.session_provider {
            spec = spec.with_relative_axes(axes.clone());
            if let Some(buttons) = buttons {
                spec = spec.with_pointing_buttons(buttons.clone())?;
            }
        }
        Ok(spec)
    }
    /// Cold host builder: pass the returned OsStrings to Command.args, never
    /// join into shell text. Uses the same strict validation as the real main.
    pub fn into_argv(self) -> Result<Vec<OsString>, ArgumentError> {
        if self.inputs.is_empty() || self.inputs.len() > MAX_INPUTS {
            return Err(ArgumentError);
        }
        let mut argv = Vec::with_capacity(9 + 2 * self.inputs.len());
        argv.extend([
            LINUX_WORKER_ARGUMENT.into(),
            self.parent.to_string().into(),
            self.run_id.to_string().into(),
            "--uinput".into(),
            self.uinput.into_os_string(),
            "--session-provider".into(),
        ]);
        match self.session_provider {
            SessionProvider::None => argv.push("none".into()),
            SessionProvider::Console { vt } => {
                argv.extend(["console".into(), "--vt".into(), vt.into_os_string()])
            }
            SessionProvider::X11 {
                helper,
                axes,
                buttons,
            } => argv.extend([
                "x11".into(),
                "--x11-probe".into(),
                helper.into_os_string(),
                "--relative-axes".into(),
                list_text(axes.codes()).into(),
                "--pointing-buttons".into(),
                buttons
                    .map_or_else(|| "none".into(), |buttons| list_text(buttons.codes()))
                    .into(),
            ]),
        }
        if self.execution_mode == ExecutionMode::StagedKeyboard {
            argv.extend(["--execution-mode".into(), "staged-keyboard".into()]);
        }
        for path in self.inputs {
            argv.extend(["--input".into(), path.into_os_string()]);
        }
        Self::parse(argv.iter().cloned())?;
        Ok(argv)
    }
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
        let (session_provider, mut start) = if args[6] == "none" {
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
        } else if args[6] == "x11"
            && args.get(7).is_some_and(|v| v == "--x11-probe")
            && args.get(9).is_some_and(|v| v == "--relative-axes")
            && args.get(11).is_some_and(|v| v == "--pointing-buttons")
        {
            let helper = PathBuf::from(args.get(8).ok_or(ArgumentError)?);
            if !helper.is_absolute() {
                return Err(ArgumentError);
            }
            let axes = keyweave_linux::relative::RelativeAxes::new(&numbers(
                args.get(10).ok_or(ArgumentError)?,
            )?)
            .map_err(|_| ArgumentError)?;
            let buttons = match args.get(12).ok_or(ArgumentError)? {
                v if v == "none" => None,
                v => Some(
                    keyweave_linux::keys::PointingButtons::new(&numbers(v)?)
                        .map_err(|_| ArgumentError)?,
                ),
            };
            if buttons.is_some() && (!axes.contains(0) || !axes.contains(1)) {
                return Err(ArgumentError);
            }
            (
                SessionProvider::X11 {
                    helper,
                    axes,
                    buttons,
                },
                13,
            )
        } else {
            return Err(ArgumentError);
        };
        let execution_mode = if args.get(start).is_some_and(|arg| arg == "--execution-mode") {
            let mode = match args.get(start + 1).and_then(|value| value.to_str()) {
                Some("legacy") => ExecutionMode::Legacy,
                Some("staged-keyboard") => ExecutionMode::StagedKeyboard,
                _ => return Err(ArgumentError),
            };
            start += 2;
            mode
        } else {
            ExecutionMode::Legacy
        };
        if matches!(session_provider, SessionProvider::X11 { .. })
            && execution_mode != ExecutionMode::StagedKeyboard
        {
            return Err(ArgumentError);
        }
        if args.len() <= start {
            return Err(ArgumentError);
        }
        let uinput = PathBuf::from(&args[4]);
        if !uinput.is_absolute()
            || matches!(&session_provider,SessionProvider::X11{helper,..} if *helper==uinput)
        {
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
                || matches!(&session_provider,SessionProvider::X11{helper,..} if *helper==input)
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
            execution_mode,
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
    #[test]
    fn builder_preserves_fixed_contract_and_literal_paths_with_spaces() {
        for provider in [
            SessionProvider::None,
            SessionProvider::Console {
                vt: "/dev/tty2".into(),
            },
        ] {
            let a = LinuxArguments {
                parent: 42,
                run_id: 57,
                execution_mode: ExecutionMode::Legacy,
                uinput: std::env::temp_dir().join("output 'literal'"),
                inputs: vec![std::env::temp_dir().join("input $literal space")],
                session_provider: provider.clone(),
            };
            let output = a.uinput.clone();
            let input = a.inputs[0].clone();
            let argv = a.into_argv().unwrap();
            assert_eq!(argv[0], LINUX_WORKER_ARGUMENT);
            assert_eq!(argv[3], "--uinput");
            assert_eq!(argv[4], output.as_os_str());
            let parsed = LinuxArguments::parse(argv).unwrap();
            assert_eq!(parsed.inputs, vec![input]);
            assert_eq!(parsed.session_provider, provider);
        }
    }
    #[test]
    fn builder_rejects_zero_identity_and_duplicate_inputs_before_spawn() {
        for (parent, run_id, duplicate) in [(0, 57, false), (42, 0, false), (42, 57, true)] {
            let input = std::env::temp_dir().join("input");
            let a = LinuxArguments {
                parent,
                run_id,
                execution_mode: ExecutionMode::Legacy,
                uinput: std::env::temp_dir().join("output"),
                inputs: if duplicate {
                    vec![input.clone(), input]
                } else {
                    vec![input]
                },
                session_provider: SessionProvider::None,
            };
            assert!(a.into_argv().is_err());
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn native_run_and_non_unicode_path_round_trip_without_process_or_input() {
        use std::os::unix::ffi::OsStringExt;
        let identity = keyweave_linux::run_identity::RunIdentity::generate().unwrap();
        let input = PathBuf::from(OsString::from_vec(b"/tmp/input-\xff".to_vec()));
        let argv = LinuxArguments {
            parent: std::process::id(),
            run_id: identity.run_id(),
            execution_mode: ExecutionMode::Legacy,
            uinput: "/dev/uinput".into(),
            inputs: vec![input.clone()],
            session_provider: SessionProvider::None,
        }
        .into_argv()
        .unwrap();
        let parsed = LinuxArguments::parse(argv).unwrap();
        assert!(parsed.run_id == identity.run_id());
        assert_eq!(parsed.inputs, vec![input]);
    }
}

#[cfg(test)]
mod execution_mode_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn argv(mode: ExecutionMode, count: usize) -> Vec<OsString> {
        LinuxArguments {
            parent: 42,
            run_id: 57,
            execution_mode: mode,
            uinput: std::env::temp_dir().join("absent-output"),
            inputs: (0..count)
                .map(|i| std::env::temp_dir().join(format!("absent-input-{i}")))
                .collect(),
            session_provider: SessionProvider::None,
        }
        .into_argv()
        .unwrap()
    }
    #[test]
    fn staged_fixed_flag_roundtrips_and_default_legacy_argv_remains_unchanged() {
        for mode in [ExecutionMode::Legacy, ExecutionMode::StagedKeyboard] {
            let args = argv(mode, 64);
            assert_eq!(
                LinuxArguments::parse(args.clone()).unwrap().execution_mode,
                mode
            );
            assert_eq!(
                args.iter().filter(|arg| *arg == "--execution-mode").count(),
                usize::from(mode == ExecutionMode::StagedKeyboard)
            );
            assert_eq!(
                LinuxArguments::parse(args).unwrap().session_provider,
                SessionProvider::None
            );
        }
    }
    #[test]
    fn mode_unknown_duplicate_and_reordered_flag_reject() {
        let mut wrong = argv(ExecutionMode::StagedKeyboard, 1);
        wrong[8] = "auto".into();
        assert!(LinuxArguments::parse(wrong).is_err());
        let mut duplicate = argv(ExecutionMode::StagedKeyboard, 1);
        duplicate.extend(["--execution-mode".into(), "legacy".into()]);
        assert!(LinuxArguments::parse(duplicate).is_err());
        let mut reordered = argv(ExecutionMode::StagedKeyboard, 1);
        reordered.swap(5, 7);
        assert!(LinuxArguments::parse(reordered).is_err());
        assert!(
            LinuxArguments::parse(argv(ExecutionMode::StagedKeyboard, 1).into_iter().take(9))
                .is_err()
        );
    }
    fn x11_args() -> Vec<OsString> {
        let mut value = argv(ExecutionMode::Legacy, 1);
        value.truncate(7);
        value[6] = "x11".into();
        value.extend([
            "--x11-probe".into(),
            std::env::temp_dir().join("private-probe").into(),
            "--relative-axes".into(),
            "0,1,8".into(),
            "--pointing-buttons".into(),
            "272,273".into(),
            "--execution-mode".into(),
            "staged-keyboard".into(),
            "--input".into(),
            std::env::temp_dir().join("private-input").into(),
        ]);
        value
    }
    #[test]
    fn x11_roundtrip_binds_staged_scope_axes_and_buttons_without_debug_paths() {
        let argv = x11_args();
        let parsed = LinuxArguments::parse(argv.clone()).unwrap();
        assert_eq!(
            parsed.capture_scope(),
            crate::linux_preparation::CaptureScope::StagedRelative
        );
        assert_eq!(parsed.execution_mode, ExecutionMode::StagedKeyboard);
        let spec = parsed.output_spec().unwrap();
        assert_eq!(
            spec.relative_axes().unwrap().codes().collect::<Vec<_>>(),
            vec![0, 1, 8]
        );
        assert_eq!(
            spec.pointing_buttons().unwrap().codes().collect::<Vec<_>>(),
            vec![272, 273]
        );
        assert!(!format!("{parsed:?}").contains("private"));
        assert_eq!(parsed.into_argv().unwrap(), argv);
    }
    #[test]
    fn x11_rejects_wrong_order_unbounded_noncanonical_or_unsupported_sets() {
        for (index, value) in [
            (7, "--helper"),
            (8, "relative"),
            (9, "--axes"),
            (10, ""),
            (10, "1,0"),
            (10, "0,0"),
            (10, "00,1"),
            (10, "0,1,2"),
            (11, "--buttons"),
            (12, "0"),
            (12, "272,272"),
            (12, "273,272"),
            (12, "65536"),
            (14, "legacy"),
        ] {
            let mut argv = x11_args();
            argv[index] = value.into();
            assert!(LinuxArguments::parse(argv).is_err());
        }
        let mut argv = x11_args();
        argv[8] = argv[4].clone();
        assert!(LinuxArguments::parse(argv).is_err());
        let mut argv = x11_args();
        argv[16] = argv[8].clone();
        assert!(LinuxArguments::parse(argv).is_err());
    }
    #[test]
    fn x11_requires_explicit_staged_mode_and_xy_for_buttons() {
        let mut argv = x11_args();
        argv.drain(13..15);
        assert!(LinuxArguments::parse(argv).is_err());
        let mut argv = x11_args();
        argv[10] = "0,8".into();
        assert!(LinuxArguments::parse(argv.clone()).is_err());
        argv[12] = "none".into();
        let parsed = LinuxArguments::parse(argv).unwrap();
        assert!(parsed.output_spec().unwrap().pointing_buttons().is_none());
    }
    #[test]
    fn x11_preserves_64_input_bound_and_os_paths_with_spaces() {
        let mut argv = x11_args();
        argv.truncate(15);
        argv[8] = std::env::temp_dir()
            .join("private probe with spaces")
            .into();
        for index in 0..64 {
            argv.extend([
                "--input".into(),
                std::env::temp_dir().join(format!("input{index}")).into(),
            ]);
        }
        assert_eq!(
            LinuxArguments::parse(argv.clone()).unwrap().inputs.len(),
            64
        );
        argv.extend([
            "--input".into(),
            std::env::temp_dir().join("input64").into(),
        ]);
        assert!(LinuxArguments::parse(argv).is_err());
    }
}
