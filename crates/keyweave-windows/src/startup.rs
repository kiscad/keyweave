//! Startup observations, never an activation token or a per-device snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputSample {
    Held,
    IdleObserved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    #[error("thread impersonation is unsupported during input sampling")]
    Impersonating,
    #[error("asynchronous state must not be queried from a Hook callback")]
    InCallback,
    #[error("visible interactive window station is unavailable")]
    NotInteractive,
    #[error("thread desktop is not the input desktop")]
    InactiveDesktop,
    #[error("foreground process integrity exceeds the caller")]
    HigherIntegrity,
    #[error("foreground process is unavailable")]
    NoForeground,
    #[error("input context changed while sampling")]
    Changed,
    #[error("native metadata format is invalid")]
    Malformed,
    #[error("{call} failed: {code}")]
    Api { call: &'static str, code: u32 },
}
