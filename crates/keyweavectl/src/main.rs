mod replay;
use clap::{Parser, Subcommand, ValueEnum};
use keyweave_config::{CompileMode, CompiledConfig, MAX_CONFIG_BYTES, compile_with_mode};
use keyweave_engine::EngineState;
use keyweave_types::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Parser)]
#[command(
    version,
    about = "KeyWeave keyboard remapping: validate and deterministic offline replay"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse and compile a configuration. No scripts or device access.
    Validate {
        #[arg(long, value_enum, default_value = "legacy")]
        execution_mode: ExecutionMode,
        #[arg(long)]
        config: PathBuf,
        #[arg(long, value_enum, default_value = "offline")]
        backend: Backend,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    /// Replay a schema-versioned synthetic fixture without reading any keyboard.
    Replay {
        #[arg(long, value_enum, default_value = "legacy")]
        execution_mode: ExecutionMode,
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long = "assert")]
        assert_expected: bool,
    },
    /// Import a lossless configuration copy; writes diagnostics and refuses lossy activation.
    ImportKarabiner {
        #[arg(long, value_enum, default_value = "legacy")]
        execution_mode: ExecutionMode,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        report: PathBuf,
        #[arg(long, value_enum, default_value = "offline")]
        backend: Backend,
    },
    /// Report implementation and environment capabilities; never claim hardware validation.
    Doctor,
    /// List known physical HID key names and aliases.
    KeyNames,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ExecutionMode {
    Legacy,
    StagedKeyboard,
}
impl ExecutionMode {
    fn compile_mode(self) -> CompileMode {
        match self {
            Self::Legacy => CompileMode::Legacy,
            Self::StagedKeyboard => CompileMode::StagedKeyboard,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Backend {
    Offline,
    LinuxEvdev,
    WindowsHook,
}
impl Backend {
    fn capabilities(self) -> BackendCapabilities {
        match self {
            Self::Offline => BackendCapabilities::offline(),
            Self::LinuxEvdev => BackendCapabilities::linux(),
            Self::WindowsHook => BackendCapabilities::windows_hook(),
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    schema_version: u32,
    config: Value,
    #[serde(default)]
    inputs: Vec<InputEnvelope>,
    #[serde(default)]
    steps: Vec<replay::Step>,
    #[serde(default)]
    expected_pointers: Option<Vec<Value>>,
    #[serde(default)]
    expected: Option<Vec<OutputIntent>>,
    #[serde(default)]
    expected_state: Option<EngineState>,
    #[serde(default)]
    expected_variables: Option<std::collections::BTreeMap<String, keyweave_types::VariableValue>>,
}

fn read_bounded(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("input file exceeds 8 MiB limit".into());
    }
    Ok(String::from_utf8(bytes)?)
}
fn compile_file(path: &Path, backend: Backend, mode: ExecutionMode) -> Result<CompiledConfig> {
    let compiled = compile_with_mode(
        &read_bounded(path)?,
        &backend.capabilities(),
        ConfigGeneration(1),
        mode.compile_mode(),
    )?;
    if mode == ExecutionMode::StagedKeyboard && compiled.executable_without_loss() {
        keyweave_engine::physical_controller::PhysicalStageController::new_with_chord_model(
            Arc::new(compiled.snapshot.clone()),
        )?;
    }
    Ok(compiled)
}
fn print_json(value: impl Serialize) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer_pretty(&mut out, &value)?;
    writeln!(out)?;
    Ok(())
}
fn write_new_json(path: &Path, value: impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, &value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
fn ensure_lossless(compiled: &CompiledConfig) -> Result<()> {
    if !compiled.executable_without_loss() {
        return Err(format!(
            "{} rule(s)/sections disabled; configuration cannot be activated without loss",
            compiled.disabled_rules
        )
        .into());
    }
    Ok(())
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Validate {
            config,
            backend,
            format,
            execution_mode,
        } => {
            let compiled = compile_file(&config, backend, execution_mode)?;
            match format {
                Format::Json => print_json(
                    json!({"execution_mode":execution_mode,"valid": compiled.executable_without_loss(), "backend": backend.capabilities(), "profile": compiled.snapshot.name, "simple_rules": compiled.snapshot.simple.len(), "complex_rules": compiled.snapshot.complex.len(), "disabled_rules": compiled.disabled_rules, "diagnostics": compiled.diagnostics}),
                )?,
                Format::Text => {
                    println!(
                        "Profile: {}; {} simple, {} complex, {} disabled",
                        compiled.snapshot.name,
                        compiled.snapshot.simple.len(),
                        compiled.snapshot.complex.len(),
                        compiled.disabled_rules
                    );
                    for diag in &compiled.diagnostics {
                        println!("{} {}: {}", diag.code, diag.pointer, diag.message);
                    }
                }
            }
            ensure_lossless(&compiled)?;
        }
        Command::Replay {
            fixture,
            assert_expected,
            execution_mode,
        } => {
            let fixture: Fixture =
                serde_json::from_value(keyweave_config::parse_jsonc(&read_bounded(&fixture)?)?)?;
            let result = replay::run(&fixture, execution_mode)?;
            let outputs = result.outputs;
            let state = result.state;
            if assert_expected {
                if fixture
                    .expected_pointers
                    .as_ref()
                    .is_some_and(|expected| *expected != result.pointers)
                {
                    return Err("pointer receipt assertion mismatch".into());
                }
                if fixture
                    .expected_variables
                    .as_ref()
                    .is_some_and(|expected| *expected != result.variables)
                {
                    return Err("final variable assertion mismatch".into());
                }
                let expected = fixture
                    .expected
                    .ok_or("--assert requires expected outputs")?;
                if outputs != expected {
                    let index = outputs
                        .iter()
                        .zip(&expected)
                        .position(|(a, b)| a != b)
                        .unwrap_or(outputs.len().min(expected.len()));
                    return Err(format!(
                        "replay mismatch at output {index}: expected {:?}, actual {:?}",
                        expected.get(index),
                        outputs.get(index)
                    )
                    .into());
                }
                if fixture
                    .expected_state
                    .as_ref()
                    .is_some_and(|expected| *expected != state)
                {
                    return Err(format!(
                        "final state mismatch: expected {:?}, actual {state:?}",
                        fixture.expected_state
                    )
                    .into());
                }
            }
            print_json(
                json!({"mode":"offline_synthetic_no_device_access", "asserted":assert_expected, "outputs":outputs, "state":state, "execution_mode":execution_mode,"forwarded_pointers":result.pointers,"repeat_targets":result.repeat_targets,"next_deadline":result.next_deadline}),
            )?;
        }
        Command::ImportKarabiner {
            input,
            output,
            report,
            backend,
            execution_mode,
        } => {
            if output == report || input == output || input == report {
                return Err("input, output and report paths must differ".into());
            }
            let compiled = compile_file(&input, backend, execution_mode)?;
            write_new_json(
                &report,
                json!({"source":input, "backend":backend.capabilities(), "disabled_rules":compiled.disabled_rules, "diagnostics":compiled.diagnostics, "execution_mode":execution_mode,"activation":"never"}),
            )?;
            ensure_lossless(&compiled)?;
            write_new_json(&output, compiled.raw)?;
            println!("Wrote lossless editable copy; no configuration activated");
        }
        Command::Doctor => print_json(
            json!({"os":std::env::consts::OS, "architecture":std::env::consts::ARCH, "version":env!("CARGO_PKG_VERSION"), "implemented":["offline_replay","basic_key_mapping","jsonc","hot_snapshot_binding","output_ownership","typed_variables","held_deadlines","delayed_actions","lazy_modifiers","binding_halt","context_conditions","staged_keyboard_replay","complex_simultaneous","timed_action_queues"], "linux_input_present":Path::new("/dev/input").is_dir(), "linux_uinput_present":Path::new("/dev/uinput").exists(), "hardware_validated":false, "platform_backends_implemented":true,"native_acceptance":"unverified"}),
        )?,
        Command::KeyNames => print_json(KeyId::standard_names())?,
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("keyweavectl: {error}");
            std::process::ExitCode::from(2)
        }
    }
}
