//! Owned host fixture only: no Hook/SendInput. Epochs are synthetic.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() {
    use keyweave_progress::{ProgressClient, ProgressPhase, RequestEvent};
    use keyweave_worker::Arguments;
    std::panic::set_hook(Box::new(|_| {}));
    let args = match Arguments::parse(std::env::args_os().skip(1)) {
        Ok(v) => v,
        Err(_) => std::process::exit(20),
    };
    let mut startup = match keyweave_windows::WorkerStartup::claim(args.parent, args.run_id) {
        Ok(v) => v,
        Err(_) => std::process::exit(21),
    };
    let handle = match args.config_handle {
        Some(v) => v,
        None => std::process::exit(22),
    };
    let mut reader = match startup.claim_config_reader(handle) {
        Ok(v) => v,
        Err(_) => std::process::exit(23),
    };
    if keyweave_worker::load_config(&mut reader).is_err() || reader.close().is_err() {
        std::process::exit(24)
    }
    let (_guard, run, reader, writer) = startup.into_parts();
    let mut client = match ProgressClient::new(reader, writer, run) {
        Ok(v) => v,
        Err(_) => std::process::exit(25),
    };
    let mode = std::env::var("KEYWEAVE_HOST_FIXTURE_MODE").unwrap_or_default();
    if !["active", "standby", "stop_stall", "startup_stall"].contains(&mode.as_str()) {
        std::process::exit(26)
    }
    let mut epoch = 1u64;
    loop {
        if mode == "startup_stall" {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        match client.receive_command(epoch) {
            Ok(RequestEvent::Stop) => {
                if mode == "stop_stall" {
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                std::process::exit(0)
            }
            Ok(RequestEvent::Challenge) => {
                if mode != "standby" {
                    epoch += 1;
                }
            }
            Ok(RequestEvent::Pending) => (),
            Err(_) => std::process::exit(27),
        }
        if mode != "standby" && client.publish(epoch, ProgressPhase::Active).is_err() {
            std::process::exit(28)
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
