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

describe("i18n locales", () => {
  it("en and fr expose exactly the same keys", () => {
    expect(flattenKeys(fr).sort()).toEqual(flattenKeys(en).sort());
  });
});

  it("does not contain written paths to settings", () => {
    const allowedPathKeys = [
      "settings_audio.sample_rate_high_warning",
      "mic_toast.switched_detail",
    ];
    
    const checkPaths = (localeName: string, flattened: Record<string, string>) => {
      for (const [key, value] of Object.entries(flattened)) {
        if (allowedPathKeys.includes(key)) continue;
        
        if (value.includes("Settings >") || value.includes("Réglages >") || value.includes("→")) {
          throw new Error(`[${localeName}] Key "${key}" contains an explicit settings path: "${value}"`);
        }
      }
    };
    
    // Create flattened objects holding the actual string values
    const flattenValues = (obj: Record<string, unknown>, prefix = ""): Record<string, string> => {
      return Object.entries(obj).reduce((acc, [key, value]) => {
        if (value !== null && typeof value === "object") {
          return { ...acc, ...flattenValues(value as Record<string, unknown>, `${prefix}${key}.`) };
        }
        acc[`${prefix}${key}`] = value as string;
        return acc;
      }, {} as Record<string, string>);
    };

    const flatEn = flattenValues(en);
    const flatFr = flattenValues(fr);

    checkPaths("en", flatEn);
    checkPaths("fr", flatFr);
  });
