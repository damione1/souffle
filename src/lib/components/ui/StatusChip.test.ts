import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/svelte";
import StatusChip from "./StatusChip.svelte";

describe("StatusChip", () => {
  it("treats load_required as an action, not an in-flight load", () => {
    render(StatusChip, {
      props: { phase: "load_required", operationState: "idle" },
    });

    expect(screen.getByRole("status").textContent).toContain("Model not loaded");
    expect(screen.getByRole("status").textContent).not.toContain("Loading model");
  });

  it("keeps the busy loading label for an in-flight load", () => {
    render(StatusChip, {
      props: { phase: "load_required", operationState: "loading" },
    });

    expect(screen.getByRole("status").textContent).toContain("Loading model");
  });
});
