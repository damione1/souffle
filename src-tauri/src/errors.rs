/// App-wide error types
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Audio error: {0}")]
    Audio(String),
    #[error("Engine error: {0}")]
    Engine(#[from] crate::engine::EngineError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Recording not active")]
    NotRecording,
    #[error("Already recording")]
    AlreadyRecording,
}

impl From<AppError> for String {
    fn from(err: AppError) -> String {
        err.to_string()
    }
}
