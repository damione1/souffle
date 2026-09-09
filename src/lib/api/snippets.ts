import { commands, unwrap } from "./generated";
import type { SnippetEntry } from "../types";

export async function listSnippets(): Promise<SnippetEntry[]> {
  return unwrap(commands.listSnippets());
}

export async function addSnippet(trigger: string, expansion: string): Promise<SnippetEntry> {
  return unwrap(commands.addSnippet(trigger, expansion));
}

export async function updateSnippet(id: number, trigger: string, expansion: string): Promise<void> {
  await unwrap(commands.updateSnippet(id, trigger, expansion));
}

export async function deleteSnippet(id: number): Promise<void> {
  await unwrap(commands.deleteSnippet(id));
}
