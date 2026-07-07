import { commands, unwrap } from "./generated";
import type { DictionaryEntry } from "../types";

export async function listDictionary(): Promise<DictionaryEntry[]> {
  return unwrap(commands.listDictionary());
}

export async function addDictionaryEntry(
  term: string,
  pronunciation: string | null,
  category: string | null,
): Promise<DictionaryEntry> {
  return unwrap(commands.addDictionaryEntry(term, pronunciation, category));
}

export async function updateDictionaryEntry(
  id: number,
  term: string,
  pronunciation: string | null,
  category: string | null,
): Promise<void> {
  await unwrap(commands.updateDictionaryEntry(id, term, pronunciation, category));
}

export async function deleteDictionaryEntry(id: number): Promise<void> {
  await unwrap(commands.deleteDictionaryEntry(id));
}

export async function clearDictionary(): Promise<void> {
  await unwrap(commands.clearDictionary());
}
