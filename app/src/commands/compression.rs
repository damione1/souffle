//! Thin command boundaries over the shared compression service lifecycle.
use crate::{
    state::AppState,
    summary::compress::{CompressionAdmission, CompressionError},
    transcript::CompactTranscript,
};
use std::sync::Arc;
/// Explicit generation may install the model; regeneration keeps an old result
/// visible until a replacement passes the source/generation CAS.
pub async fn generate_compact_transcript(
    state: Arc<AppState>,
    id: String,
    rate: Option<f32>,
    regenerate: bool,
) -> Result<CompactTranscript, CompressionError> {
    state
        .compression
        .clone()
        .generate(
            state,
            id,
            rate,
            regenerate,
            CompressionAdmission::InstallIfNeeded,
        )
        .await
}
/// Summary generation reuses fresh stored output or an installed model only. It
/// never waits behind an installer or initiates/repairs a model download.
pub async fn generate_compact_transcript_ready(
    state: Arc<AppState>,
    id: String,
    rate: Option<f32>,
) -> Result<CompactTranscript, CompressionError> {
    state
        .compression
        .clone()
        .generate(state, id, rate, false, CompressionAdmission::ReadyOnly)
        .await
}
