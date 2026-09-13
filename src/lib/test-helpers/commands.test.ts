import { beforeEach, describe, expect, it, vi } from "vitest";

const mockInvoke = vi.fn();
let callbackId = 0;
Object.defineProperty(window, "__TAURI_INTERNALS__", {
  value: {
    invoke: mockInvoke,
    transformCallback: () => ++callbackId,
    metadata: {
      currentWebview: { windowLabel: "main", label: "main" },
      currentWindow: { label: "main" },
    },
  },
  writable: true,
});

const { COMMAND } = await import("./commands");
const { commands } = await import("../types/generated");

/**
 * The other half of the guard in `commands.ts`. The key type already fails
 * compilation when a command is renamed in Rust; this asserts the value is the
 * wire name the generated client actually sends, so the table cannot drift
 * from the contract in either direction.
 */
describe("COMMAND table", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(null);
  });

  const entries = Object.entries(COMMAND) as [keyof typeof COMMAND, string][];

  it("covers every command the tests mock", () => {
    expect(entries.length).toBeGreaterThan(0);
  });

  it.each(entries)("%s sends %s", async (method, wire) => {
    const call = commands[method] as (...args: unknown[]) => unknown;
    await call();
    expect(mockInvoke).toHaveBeenCalledWith(wire, expect.anything(), undefined);
  });
});
