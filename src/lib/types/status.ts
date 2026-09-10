/** Why a status banner is on screen.
 *
 * Controllers store the reason rather than a bare string so a banner can be
 * dropped the moment its condition goes away (permission granted, model
 * loaded) instead of surviving until the next session starts (SOU-089 AC6). */
export type StatusReason = {
  type: "transient" | "accessibility_denied" | "no_model";
  message: string;
  actionLabel?: string;
  onAction?: () => void;
};
