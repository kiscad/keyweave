use clap::{Parser, Subcommand, ValueEnum};
use keyweave_config::{CompiledConfig, MAX_CONFIG_BYTES, compile};
use keyweave_engine::{Engine, EngineState};
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
        #[arg(long)]
        config: PathBuf,
        #[arg(long, value_enum, default_value = "offline")]
        backend: Backend,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    /// Replay a schema-versioned synthetic fixture without reading any keyboard.
    Replay {
        #[arg(long)]
        fixture: PathBuf,
        #[arg(long = "assert")]
        assert_expected: bool,
    },
    /// Import a lossless configuration copy; writes diagnostics and refuses lossy activation.
    ImportKarabiner {
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
    inputs: Vec<InputEnvelope>,
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
fn compile_file(path: &Path, backend: Backend) -> Result<CompiledConfig> {
    Ok(compile(
        &read_bounded(path)?,
        &backend.capabilities(),
        ConfigGeneration(1),
    )?)
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
        } => {
            let compiled = compile_file(&config, backend)?;
            match format {
                Format::Json => print_json(
                    json!({"valid": compiled.executable_without_loss(), "backend": backend.capabilities(), "profile": compiled.snapshot.name, "simple_rules": compiled.snapshot.simple.len(), "complex_rules": compiled.snapshot.complex.len(), "disabled_rules": compiled.disabled_rules, "diagnostics": compiled.diagnostics}),
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
        } => {
            let fixture: Fixture =
                serde_json::from_value(keyweave_config::parse_jsonc(&read_bounded(&fixture)?)?)?;
            if fixture.schema_version != 1 {
                return Err("replay schema_version must be 1".into());
            }
            if fixture.inputs.len() > 100_000 {
                return Err("replay input limit exceeded".into());
            }
            let config = compile(
                &serde_json::to_string(&fixture.config)?,
                &BackendCapabilities::offline(),
                ConfigGeneration(1),
            )?;
            ensure_lossless(&config)?;
            let mut engine = Engine::new(Arc::new(config.snapshot))?;
            let mut outputs = Vec::new();
            for input in fixture.inputs {
                outputs.extend(engine.handle(input)?.outputs);
            }
            let state = engine.state();
            if assert_expected {
                if fixture
                    .expected_variables
                    .as_ref()
                    .is_some_and(|expected| *expected != engine.variable_values())
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
                json!({"mode":"offline_synthetic_no_device_access", "asserted":assert_expected, "outputs":outputs, "state":state}),
            )?;
        }
        Command::ImportKarabiner {
            input,
            output,
            report,
            backend,
        } => {
            if output == report || input == output || input == report {
                return Err("input, output and report paths must differ".into());
            }
            let compiled = compile_file(&input, backend)?;
            write_new_json(
                &report,
                json!({"source":input, "backend":backend.capabilities(), "disabled_rules":compiled.disabled_rules, "diagnostics":compiled.diagnostics, "activation":"never"}),
            )?;
            ensure_lossless(&compiled)?;
            write_new_json(&output, compiled.raw)?;
            println!("Wrote lossless editable copy; no configuration activated");
        }
        Command::Doctor => print_json(
            json!({"os":std::env::consts::OS, "architecture":std::env::consts::ARCH, "version":env!("CARGO_PKG_VERSION"), "implemented":["offline_replay","basic_key_mapping","jsonc","hot_snapshot_binding","output_ownership","typed_variables","held_deadlines","context_conditions"], "linux_input_present":Path::new("/dev/input").is_dir(), "linux_uinput_present":Path::new("/dev/uinput").exists(), "hardware_validated":false, "platform_backends_implemented":false}),
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
