import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import SpeakerManagePopover from "./SpeakerManagePopover.svelte";
import type { MeetingSpeaker } from "../../../types";
import type { AnchorRect } from "../../../utils";

const anchorRect: AnchorRect = { top: 10, left: 10, bottom: 20, right: 20, width: 10, height: 10 };
const otherSpeaker: MeetingSpeaker = { id: 2, name: "Bob" };

function renderPopover(onRetag: (options: {
  scope: "turn" | "meeting";
  toSpeakerId: number | null;
  newSpeakerName: string | null;
  remember: boolean;
}) => void) {
  return render(SpeakerManagePopover, {
    props: {
      speakerId: 1,
      speakerName: "Alice",
      meetingSpeakers: [otherSpeaker],
      allSpeakers: [otherSpeaker],
      anchorRect,
      onClose: vi.fn(),
      onRename: vi.fn(),
      onRetag,
    },
  });
}

function selectMeetingScope() {
  return fireEvent.click(screen.getByText("All turns in this meeting"));
}

describe("SpeakerManagePopover", () => {
  it("has no remember checkbox for a turn-scoped retag and sends remember=false", async () => {
    const onRetag = vi.fn();
    renderPopover(onRetag);

    // Default scope is "This turn only".
    expect(screen.queryByRole("checkbox")).toBeNull();

    await fireEvent.click(screen.getByText("Apply reassignment"));

    expect(onRetag).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "turn", toSpeakerId: 2, remember: false }),
    );
  });

  it("remembers the voice by default when retagging the whole meeting", async () => {
    const onRetag = vi.fn();
    renderPopover(onRetag);

    await selectMeetingScope();

    expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(screen.getByText("Apply reassignment"));

    expect(onRetag).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "meeting", toSpeakerId: 2, remember: true }),
    );
  });

  it("does not remember the voice when the checkbox is unchecked in meeting scope", async () => {
    const onRetag = vi.fn();
    renderPopover(onRetag);

    await selectMeetingScope();
    await fireEvent.click(screen.getByRole("checkbox"));
    expect(screen.getByText("Only changes this meeting.")).toBeTruthy();

    await fireEvent.click(screen.getByText("Apply reassignment"));

    expect(onRetag).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "meeting", toSpeakerId: 2, remember: false }),
    );
  });

  it("hides the checkbox again when switching back to turn scope", async () => {
    renderPopover(vi.fn());

    await selectMeetingScope();
    expect(screen.getByRole("checkbox")).toBeTruthy();

    await fireEvent.click(screen.getByText("This turn only"));

    expect(screen.queryByRole("checkbox")).toBeNull();
  });
});
