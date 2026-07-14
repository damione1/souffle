//! Smart meeting start/stop UX: consumes meeting-detect signals and emits
//! coalesced start nudges plus strong end nudges during recording.

mod coordinator;

pub use coordinator::spawn;
