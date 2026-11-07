//! Exhaustive domain/UI conversions live with the generated UI types.
use souffle_lib::engine::TranscriptionRuntimePhase;

impl From<TranscriptionRuntimePhase> for crate::TranscriptionPhase {
    fn from(phase: TranscriptionRuntimePhase) -> Self {
        match phase {
            TranscriptionRuntimePhase::DownloadRequired => Self::DownloadRequired,
            TranscriptionRuntimePhase::Downloading => Self::Downloading,
            TranscriptionRuntimePhase::LoadRequired => Self::LoadRequired,
            TranscriptionRuntimePhase::Loading => Self::Loading,
            TranscriptionRuntimePhase::Ready => Self::Ready,
            TranscriptionRuntimePhase::Unloading => Self::Unloading,
            TranscriptionRuntimePhase::Failed => Self::Failed,
            TranscriptionRuntimePhase::AudioFailed => Self::AudioFailed,
        }
    }
}

impl From<crate::TranscriptionPhase> for TranscriptionRuntimePhase {
    fn from(phase: crate::TranscriptionPhase) -> Self {
        match phase {
            crate::TranscriptionPhase::DownloadRequired => Self::DownloadRequired,
            crate::TranscriptionPhase::Downloading => Self::Downloading,
            crate::TranscriptionPhase::LoadRequired => Self::LoadRequired,
            crate::TranscriptionPhase::Loading => Self::Loading,
            crate::TranscriptionPhase::Ready => Self::Ready,
            crate::TranscriptionPhase::Unloading => Self::Unloading,
            crate::TranscriptionPhase::Failed => Self::Failed,
            crate::TranscriptionPhase::AudioFailed => Self::AudioFailed,
        }
    }
}

impl From<souffle_lib::engine::TranscriptionUnavailableReason>
    for crate::TranscriptionUnavailableReason
{
    fn from(reason: souffle_lib::engine::TranscriptionUnavailableReason) -> Self {
        use souffle_lib::engine::TranscriptionUnavailableReason as Domain;
        match reason {
            Domain::OsUnsupported => Self::OsUnsupported,
            Domain::DeviceUnsupported => Self::DeviceUnsupported,
            Domain::BuildUnsupported => Self::BuildUnsupported,
            Domain::LocaleUnsupported => Self::LocaleUnsupported,
            Domain::AssetsUnsupported => Self::AssetsUnsupported,
            Domain::CheckFailed => Self::CheckFailed,
        }
    }
}

impl From<crate::TranscriptionUnavailableReason>
    for souffle_lib::engine::TranscriptionUnavailableReason
{
    fn from(reason: crate::TranscriptionUnavailableReason) -> Self {
        match reason {
            crate::TranscriptionUnavailableReason::OsUnsupported => Self::OsUnsupported,
            crate::TranscriptionUnavailableReason::DeviceUnsupported => Self::DeviceUnsupported,
            crate::TranscriptionUnavailableReason::BuildUnsupported => Self::BuildUnsupported,
            crate::TranscriptionUnavailableReason::LocaleUnsupported => Self::LocaleUnsupported,
            crate::TranscriptionUnavailableReason::AssetsUnsupported => Self::AssetsUnsupported,
            crate::TranscriptionUnavailableReason::CheckFailed => Self::CheckFailed,
        }
    }
}

impl From<crate::SystemSettingsPane> for souffle_lib::commands::SystemSettingsPane {
    fn from(pane: crate::SystemSettingsPane) -> Self {
        match pane {
            crate::SystemSettingsPane::AppleIntelligence => Self::AppleIntelligence,
            crate::SystemSettingsPane::LanguageAndRegion => Self::LanguageAndRegion,
            crate::SystemSettingsPane::SoftwareUpdate => Self::SoftwareUpdate,
        }
    }
}

impl From<souffle_lib::commands::SystemSettingsPane> for crate::SystemSettingsPane {
    fn from(pane: souffle_lib::commands::SystemSettingsPane) -> Self {
        match pane {
            souffle_lib::commands::SystemSettingsPane::AppleIntelligence => Self::AppleIntelligence,
            souffle_lib::commands::SystemSettingsPane::LanguageAndRegion => Self::LanguageAndRegion,
            souffle_lib::commands::SystemSettingsPane::SoftwareUpdate => Self::SoftwareUpdate,
        }
    }
}
