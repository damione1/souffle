//! The "microphone not responding" banner (SOU-126 AC5): which stuck device
//! to name, why, and whether the user already dismissed it.
//!
//! The capture thread publishes a snapshot
//! (`souffle_lib::commands::get_mic_stall_notice`) and dispatches
//! `NativeAction::MicStallChanged` on each change, when the window re-reads it;
//! this module turns that snapshot into the Window's `mic-stall-*` pair,
//! which only [`MicStallBanner::project`] writes.

use crate::{MainWindow, MicStall};
use souffle_lib::audio::capture::{MicStallKind, MicStallNotice};

pub fn mic_stall_to_slint(kind: MicStallKind) -> MicStall {
    match kind {
        MicStallKind::TimedOut => MicStall::TimedOut,
        MicStallKind::Parked => MicStall::Parked,
        MicStallKind::Busy => MicStall::Busy,
    }
}

/// What the banner shows, `None` when hidden.
#[derive(Debug, Clone, PartialEq)]
pub struct MicStallView {
    pub kind: MicStall,
    pub device_name: String,
}

/// Dismissal is per device: the capture thread re-reports the same stuck
/// device as it cycles through timeout, park and busy, and each report must
/// not bring back a banner the user closed. A different device, or the
/// device coming back and sticking again later, shows it again.
#[derive(Debug, Default)]
pub struct MicStallBanner {
    dismissed_device: Option<String>,
}

impl MicStallBanner {
    pub fn view(&mut self, notice: Option<&MicStallNotice>) -> Option<MicStallView> {
        let Some(notice) = notice else {
            self.dismissed_device = None;
            return None;
        };
        if self.dismissed_device.as_deref() == Some(notice.device_name.as_str()) {
            return None;
        }
        self.dismissed_device = None;
        Some(MicStallView {
            kind: mic_stall_to_slint(notice.kind),
            device_name: notice.device_name.clone(),
        })
    }

    pub fn dismiss(&mut self, notice: Option<&MicStallNotice>) {
        self.dismissed_device = notice.map(|n| n.device_name.clone());
    }

    /// Write the Window's banner properties, only when they change.
    pub fn project(&mut self, window: &MainWindow, notice: Option<&MicStallNotice>) {
        match self.view(notice) {
            None => {
                if window.get_mic_stall_visible() {
                    window.set_mic_stall_visible(false);
                }
            }
            Some(view) => {
                if window.get_mic_stall_kind() != view.kind {
                    window.set_mic_stall_kind(view.kind);
                }
                if window.get_mic_stall_device().as_str() != view.device_name {
                    window.set_mic_stall_device(view.device_name.into());
                }
                if !window.get_mic_stall_visible() {
                    window.set_mic_stall_visible(true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(kind: MicStallKind, device: &str) -> MicStallNotice {
        MicStallNotice {
            kind,
            device_name: device.into(),
        }
    }

    #[test]
    fn every_stall_kind_reaches_the_ui_as_its_own_variant() {
        assert_eq!(
            mic_stall_to_slint(MicStallKind::TimedOut),
            MicStall::TimedOut
        );
        assert_eq!(mic_stall_to_slint(MicStallKind::Parked), MicStall::Parked);
        assert_eq!(mic_stall_to_slint(MicStallKind::Busy), MicStall::Busy);
    }

    #[test]
    fn a_busy_device_is_named_in_the_banner() {
        let mut banner = MicStallBanner::default();
        assert_eq!(
            banner.view(Some(&notice(MicStallKind::Busy, "Dock Mic"))),
            Some(MicStallView {
                kind: MicStall::Busy,
                device_name: "Dock Mic".into(),
            })
        );
        assert_eq!(banner.view(None), None);
    }

    #[test]
    fn a_dismissed_device_stays_hidden_until_another_one_sticks() {
        let mut banner = MicStallBanner::default();
        let dock_busy = notice(MicStallKind::Busy, "Dock Mic");
        banner.dismiss(Some(&dock_busy));
        assert_eq!(banner.view(Some(&dock_busy)), None);
        assert_eq!(
            banner.view(Some(&notice(MicStallKind::Parked, "Dock Mic"))),
            None,
            "the same device cycling through park and busy stays dismissed"
        );
        assert!(
            banner
                .view(Some(&notice(MicStallKind::TimedOut, "AirPods")))
                .is_some()
        );
    }

    #[test]
    fn a_recovered_device_that_sticks_again_is_shown_again() {
        let mut banner = MicStallBanner::default();
        let dock_busy = notice(MicStallKind::Busy, "Dock Mic");
        banner.dismiss(Some(&dock_busy));
        assert_eq!(banner.view(None), None);
        assert!(banner.view(Some(&dock_busy)).is_some());
    }
}
