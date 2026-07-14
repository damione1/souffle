import { describe, expect, it, vi } from "vitest";
import { fixedPopoverStyle } from "./portal";

describe("fixedPopoverStyle", () => {
  it("places the panel below the anchor when there is room", () => {
    vi.stubGlobal("innerHeight", 800);
    vi.stubGlobal("innerWidth", 1200);
    expect(
      fixedPopoverStyle(
        { top: 100, left: 40, bottom: 120, right: 80, width: 40, height: 20 },
        { width: 288, estimatedHeight: 280 },
      ),
    ).toBe("top:126px;left:40px;width:288px");
  });

  it("flips above the anchor when the viewport bottom is tight", () => {
    vi.stubGlobal("innerHeight", 400);
    vi.stubGlobal("innerWidth", 1200);
    expect(
      fixedPopoverStyle(
        { top: 350, left: 40, bottom: 370, right: 80, width: 40, height: 20 },
        { width: 288, estimatedHeight: 280 },
      ),
    ).toBe("top:64px;left:40px;width:288px");
  });

  it("clamps left so the panel stays on screen", () => {
    vi.stubGlobal("innerHeight", 800);
    vi.stubGlobal("innerWidth", 300);
    expect(
      fixedPopoverStyle(
        { top: 100, left: 200, bottom: 120, right: 240, width: 40, height: 20 },
        { width: 288, estimatedHeight: 280 },
      ),
    ).toBe("top:126px;left:8px;width:288px");
  });
});
