import type { InstallBlockReason } from "../../api/updater";
import type { AppStateMachine } from "../../types";

/** Install block for every machine state, keyed by the contract's own state
 * union. Adding a state to `AppStateMachine` in Rust fails `npm run check`
 * here, the same way it fails `cargo build` in `install_blocked_reason`.
 *
 * The backend stays authoritative: `getUpdateInstallBlock()` overwrites this
 * verdict as soon as it answers, and `install_update` refuses again before any
 * side effect. This table is what keeps the button from being active during
 * the round trip, or when the command fails. */
const INSTALL_BLOCK_BY_STATE: Record<
  AppStateMachine["state"],
  InstallBlockReason | null
> = {
  idle: null,
  downloading: "downloading",
  downloaded: null,
  loading: "loading",
  ready: null,
  recording_dictation: "recording_dictation",
  recording_meeting: "recording_meeting",
  stopping: "stopping",
  unloading: "unloading",
  error: null,
};

export function blockFromMachine(
  state: AppStateMachine["state"],
): InstallBlockReason | null {
  return INSTALL_BLOCK_BY_STATE[state];
}
