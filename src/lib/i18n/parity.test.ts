import { describe, expect, it } from "vitest";
import en from "./en.json";
import fr from "./fr.json";

function flattenKeys(obj: Record<string, unknown>, prefix = ""): string[] {
  return Object.entries(obj).flatMap(([key, value]) =>
    value !== null && typeof value === "object"
      ? flattenKeys(value as Record<string, unknown>, `${prefix}${key}.`)
      : [`${prefix}${key}`],
  );
}

/** Every Svelte component and controller in the tree, as raw source. */
const sources = import.meta.glob<string>(["/src/**/*.svelte", "/src/**/*.ts", "!/src/**/*.test.ts"], {
  query: "?raw",
  import: "default",
  eager: true,
});

/** Literal keys passed to `$t("…")` in Svelte, `tr("…")` or `get(t)("…")` in
 * controllers. Template literals and variables (`$t(\`x.${y}\`)`,
 * `$t(keys[option])`) are not resolvable statically and stay out of scope. */
const LITERAL_KEY = /(?:\$t|\btr|get\(t\))\(\s*"([^"]+)"/g;

function referencedKeys(): Map<string, Set<string>> {
  const byKey = new Map<string, Set<string>>();
  for (const [file, source] of Object.entries(sources)) {
    for (const match of source.matchAll(LITERAL_KEY)) {
      const key = match[1];
      const files = byKey.get(key) ?? new Set<string>();
      files.add(file);
      byKey.set(key, files);
    }
  }
  return byKey;
}

describe("i18n locales", () => {
  it("en and fr expose exactly the same keys", () => {
    expect(flattenKeys(fr).sort()).toEqual(flattenKeys(en).sort());
  });

  // Parity alone never caught a component reading a key neither locale
  // defines (SOU-036 shipped `settings_system.*` against a JSON that only had
  // `settings_startup.*`). Every literal key used in the tree must resolve.
  it("every literal key referenced in src resolves in en", () => {
    const known = new Set(flattenKeys(en));
    const referenced = referencedKeys();
    expect(referenced.size).toBeGreaterThan(0);
    const missing = [...referenced]
      .filter(([key]) => !known.has(key))
      .map(([key, files]) => `${key} (${[...files].join(", ")})`);
    expect(missing).toEqual([]);
  });
});
