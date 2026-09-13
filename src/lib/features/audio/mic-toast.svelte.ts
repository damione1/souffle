import type { InputRouteNotice, InputRouteReason } from "../../types";

const AUTO_HIDE_MS = 5_000;

export type MicToastCopy = {
  title: string;
  detail: string;
  hint: string;
  hasAction: boolean;
};

export type Translate = (key: string, options?: { values?: Record<string, string | number> }) => string;

export function micToastCopy(notice: InputRouteNotice, t: Translate): MicToastCopy {
  switch (notice.reason) {
    case "switched":
      return {
        title: t("mic_toast.switched"),
        detail: t("mic_toast.switched_detail", {
          values: { from: notice.from_name ?? "", to: notice.to_name ?? "" },
        }),
        hint: t("mic_toast.switched_hint"),
        hasAction: false,
      };
    case "connected": {
      const isBt = notice.transport === "bluetooth" || notice.transport === "bluetooth_le";
      return {
        title: t("mic_toast.connected"),
        detail: t("mic_toast.connected_detail", { values: { name: notice.to_name ?? "" } }),
        hint: t(isBt ? "mic_toast.connected_hint_bt" : "mic_toast.connected_hint"),
        hasAction: true,
      };
    }
    case "lost":
      return {
        title: t("mic_toast.lost"),
        detail: notice.from_name
          ? t("mic_toast.lost_detail", { values: { name: notice.from_name } })
          : t("mic_toast.lost_none"),
        hint: t("mic_toast.lost_hint"),
        hasAction: notice.from_name == null,
      };
  }
}

function isSameLost(a: InputRouteNotice, b: InputRouteNotice): boolean {
  return a.reason === "lost" && b.reason === "lost" && a.from_name === b.from_name;
}

/** Whether a notice is merely informational (auto-hides) or needs the user to
 * see it. Keyed on the union so a new reason has to be classified here rather
 * than silently inheriting "needs attention". */
const IS_INFORMATIONAL: Record<InputRouteReason, boolean> = {
  switched: false,
  connected: true,
  lost: false,
};

function isInformational(reason: InputRouteReason): boolean {
  return IS_INFORMATIONAL[reason];
}

export function createMicToast(hideMs = AUTO_HIDE_MS) {
  let current = $state<InputRouteNotice | null>(null);
  let pending: InputRouteNotice | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function clearTimer() {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  function scheduleHide() {
    clearTimer();
    timer = setTimeout(advance, hideMs);
  }

  function advance() {
    clearTimer();
    if (pending) {
      current = pending;
      pending = null;
      scheduleHide();
      return;
    }
    current = null;
  }

  return {
    get current() {
      return current;
    },
    show(notice: InputRouteNotice) {
      if (current && isSameLost(current, notice)) {
        return;
      }
      if (pending && isSameLost(pending, notice)) {
        return;
      }
      // Switched/Lost stay on screen; a simultaneous Connected (USB + BT
      // plug) waits until they auto-hide instead of last-wins overwriting.
      if (current && isInformational(notice.reason) && !isInformational(current.reason)) {
        pending = notice;
        return;
      }
      current = notice;
      scheduleHide();
    },
    dismiss() {
      clearTimer();
      pending = null;
      current = null;
    },
  };
}

export const micToast = createMicToast();
