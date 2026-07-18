import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/svelte";
import type { SpeakerProfile } from "../../../types";

const mockListSpeakerProfiles = vi.fn<() => Promise<SpeakerProfile[]>>();
const mockMergeSpeakers = vi.fn<(sourceId: number, targetId: number) => Promise<void>>();
const mockSetSpeakerIsMe = vi.fn<(id: number, isMe: boolean) => Promise<void>>();
const mockRenameSpeaker = vi.fn<(id: number, name: string) => Promise<void>>();
const mockDeleteSpeaker = vi.fn<(id: number) => Promise<void>>();

vi.mock("../../../api/speakers", () => ({
  listSpeakerProfiles: (...a: unknown[]) => mockListSpeakerProfiles(...(a as [])),
  mergeSpeakers: (...a: unknown[]) => mockMergeSpeakers(...(a as [number, number])),
  setSpeakerIsMe: (...a: unknown[]) => mockSetSpeakerIsMe(...(a as [number, boolean])),
  renameSpeaker: (...a: unknown[]) => mockRenameSpeaker(...(a as [number, string])),
  deleteSpeaker: (...a: unknown[]) => mockDeleteSpeaker(...(a as [number])),
}));

import SpeakersListSettingsSection from "./SpeakersListSettingsSection.svelte";

function makeProfile(overrides: Partial<SpeakerProfile> = {}): SpeakerProfile {
  return {
    id: 1,
    name: "Alice",
    last_seen_at: "2026-07-01T10:00:00Z",
    meeting_count: 3,
    segment_count: 42,
    is_me: false,
    ...overrides,
  };
}

describe("SpeakersListSettingsSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockRenameSpeaker.mockResolvedValue(undefined);
    mockDeleteSpeaker.mockResolvedValue(undefined);
    mockSetSpeakerIsMe.mockResolvedValue(undefined);
    mockMergeSpeakers.mockResolvedValue(undefined);
  });

  it("marks a speaker as me and refreshes the list", async () => {
    const alice = makeProfile({ id: 1, name: "Alice", is_me: false });
    mockListSpeakerProfiles.mockResolvedValue([alice]);

    render(SpeakersListSettingsSection);
    await screen.findByText("Alice");

    mockListSpeakerProfiles.mockResolvedValue([{ ...alice, is_me: true }]);
    await fireEvent.click(screen.getByText("Mark as me"));

    expect(mockSetSpeakerIsMe).toHaveBeenCalledWith(1, true);
    await screen.findByText("Me");
    expect(mockListSpeakerProfiles).toHaveBeenCalledTimes(2);
  });

  it("unmarks a speaker that is currently me", async () => {
    const alice = makeProfile({ id: 1, name: "Alice", is_me: true });
    mockListSpeakerProfiles.mockResolvedValue([alice]);

    render(SpeakersListSettingsSection);
    await screen.findByText("Alice");

    mockListSpeakerProfiles.mockResolvedValue([{ ...alice, is_me: false }]);
    await fireEvent.click(screen.getByText("Unmark as me"));

    expect(mockSetSpeakerIsMe).toHaveBeenCalledWith(1, false);
    await vi.waitFor(() => {
      expect(screen.queryByText("Me")).toBeNull();
    });
  });

  it("merges a speaker into the selected target and refreshes the list", async () => {
    const alice = makeProfile({ id: 1, name: "Alice" });
    const bob = makeProfile({ id: 2, name: "Bob" });
    mockListSpeakerProfiles.mockResolvedValue([alice, bob]);

    render(SpeakersListSettingsSection);
    await screen.findByText("Alice");

    const aliceItem = screen.getAllByRole("listitem")[0];
    await fireEvent.click(within(aliceItem).getByText("Merge"));

    // The panel defaults to the only other speaker (Bob) as target.
    await within(aliceItem).findByText("Merge Alice into:", { exact: false });

    mockListSpeakerProfiles.mockResolvedValue([bob]);
    await fireEvent.click(within(aliceItem).getByText("Merge"));
    await fireEvent.click(within(aliceItem).getByText("Merge"));

    expect(mockMergeSpeakers).toHaveBeenCalledWith(1, 2);
    await vi.waitFor(() => {
      expect(mockListSpeakerProfiles).toHaveBeenCalledTimes(2);
    });
  });

  it("drops a stale merge target on refresh instead of merging into it", async () => {
    const alice = makeProfile({ id: 1, name: "Alice" });
    const bob = makeProfile({ id: 2, name: "Bob" });
    const carol = makeProfile({ id: 3, name: "Carol" });
    mockListSpeakerProfiles.mockResolvedValue([alice, bob]);

    render(SpeakersListSettingsSection);
    await screen.findByText("Alice");

    const aliceItem = screen.getAllByRole("listitem")[0];
    await fireEvent.click(within(aliceItem).getByText("Merge"));
    // Defaults to the only other speaker, Bob, as the merge target.
    await within(aliceItem).findByText("Merge Alice into:", { exact: false });

    // Bob disappears from a refresh triggered while the panel is still
    // open (e.g. another row's action); Carol is now the only candidate.
    mockListSpeakerProfiles.mockResolvedValue([alice, carol]);
    await fireEvent.click(within(aliceItem).getByText("Mark as me"));
    await vi.waitFor(() => {
      expect(mockListSpeakerProfiles).toHaveBeenCalledTimes(2);
    });

    const refreshedAliceItem = screen.getAllByRole("listitem")[0];
    await within(refreshedAliceItem).findByText("Merge Alice into:", { exact: false });
    await fireEvent.click(within(refreshedAliceItem).getByText("Merge"));
    await fireEvent.click(within(refreshedAliceItem).getByText("Merge"));

    expect(mockMergeSpeakers).toHaveBeenCalledWith(1, 3);
    expect(mockMergeSpeakers).not.toHaveBeenCalledWith(1, 2);
  });

  it("closes the merge panel on refresh if its source speaker disappears", async () => {
    const alice = makeProfile({ id: 1, name: "Alice" });
    const bob = makeProfile({ id: 2, name: "Bob" });
    mockListSpeakerProfiles.mockResolvedValue([alice, bob]);

    render(SpeakersListSettingsSection);
    await screen.findByText("Alice");

    const aliceItem = screen.getAllByRole("listitem")[0];
    await fireEvent.click(within(aliceItem).getByText("Merge"));
    await within(aliceItem).findByText("Merge Alice into:", { exact: false });

    // Alice (the merge source) is deleted from elsewhere; the next refresh
    // must close the panel rather than leave it pointed at a dead source.
    mockListSpeakerProfiles.mockResolvedValue([bob]);
    await fireEvent.click(within(aliceItem).getByText("Mark as me"));

    await vi.waitFor(() => {
      expect(screen.queryByText("Merge Alice into:", { exact: false })).toBeNull();
    });
    expect(mockMergeSpeakers).not.toHaveBeenCalled();
  });

  it("does not show a merge action when only one speaker is remembered", async () => {
    mockListSpeakerProfiles.mockResolvedValue([makeProfile({ id: 1, name: "Alice" })]);

    render(SpeakersListSettingsSection);
    await screen.findByText("Alice");

    expect(screen.queryByText("Merge")).toBeNull();
  });

  it("shows an error message when refresh fails", async () => {
    mockListSpeakerProfiles.mockRejectedValue(new Error("db offline"));

    render(SpeakersListSettingsSection);

    await screen.findByText("db offline");
  });
});
