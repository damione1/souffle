//! Thin command boundaries over the shared compression service lifecycle.
use crate::{
    state::AppState,
    summary::compress::{CompressionAdmission, CompressionError},
    transcript::CompactTranscript,
};
use std::sync::Arc;
/// Native requests carry the opt-in generation captured before dispatch.
pub fn generate_compact_transcript_native(
    state: Arc<AppState>,
    id: String,
    regenerate: bool,
) -> impl std::future::Future<Output = Result<CompactTranscript, CompressionError>>
+ Send
+ 'static
+ use<> {
    let service = state.compression.clone();
    let intent = service.native_install_intent();
    service.generate(
        state,
        id,
        regenerate,
        CompressionAdmission::Native { intent },
    )
}
/// Explicit generation may install the model; regeneration keeps an old result
/// visible until a replacement passes the source/generation CAS.
pub fn generate_compact_transcript(
    state: Arc<AppState>,
    id: String,
    regenerate: bool,
) -> impl std::future::Future<Output = Result<CompactTranscript, CompressionError>>
+ Send
+ 'static
+ use<> {
    state
        .compression
        .clone()
        .generate(state, id, regenerate, CompressionAdmission::InstallIfNeeded)
}
/// Summary generation reuses fresh stored output or an installed model only. It
/// never waits behind an installer or initiates/repairs a model download.
pub async fn generate_compact_transcript_ready(
    state: Arc<AppState>,
    id: String,
) -> Result<CompactTranscript, CompressionError> {
    state
        .compression
        .clone()
        .generate(state, id, false, CompressionAdmission::ReadyOnly)
        .await
}
