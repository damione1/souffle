import { commands, unwrap } from "./generated";

export async function frontmostAppName(): Promise<string | null> {
  return unwrap(commands.frontmostAppName());
}

export async function readSelectedText(): Promise<string | null> {
  return unwrap(commands.readSelectedText());
}

export async function readFocusedText(): Promise<string | null> {
  return unwrap(commands.readFocusedText());
}
