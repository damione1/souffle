/// Audio pipeline sample rate (Mimi codec expects 24kHz)
pub const SAMPLE_RATE: u32 = 24_000;

/// Sample rate as f64 for duration calculations
pub const SAMPLE_RATE_F64: f64 = 24_000.0;

/// Milliseconds to wait for audio thread to flush its buffers after stop
pub const AUDIO_FLUSH_MS: u64 = 300;

/// Timeout for pipeline drain/flush on stop (seconds)
pub const PIPELINE_DRAIN_TIMEOUT_SECS: u64 = 5;

/// 1.5 seconds of silence at 24kHz — used as suffix for flushing
pub const SILENCE_SUFFIX_SAMPLES: usize = 36_000;

/// Mimi codec frame size: 1920 samples = 80ms at 24kHz
pub const MIMI_FRAME_SIZE: usize = 1920;
