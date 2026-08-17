use thiserror::Error;

#[derive(Error, Debug)]
pub enum IrisError {
    #[error("config error: {0}")]
    Config(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("device error: {0}")]
    Device(String),
    #[error("capture error: {0}")]
    Capture(String),
    #[error("stream error: {0}")]
    Stream(String),
    #[error("control error: {0}")]
    Control(String),
    #[error("ipc error: {0}")]
    Ipc(String),
}

pub type IrisResult<T> = Result<T, IrisError>;
