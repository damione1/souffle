import { addDictionaryEntry } from "../../api/dictionary";

export async function seedDictionary(answer: string): Promise<void> {
  const terms = answer.split(/[,;]+/).map(t => t.trim()).filter(Boolean);
  for (const term of terms) {
    try {
      await addDictionaryEntry(term, null, "interview");
    } catch (e) {
      // Ignore unique constraint or other errors silently as requested
    }
  }
}
