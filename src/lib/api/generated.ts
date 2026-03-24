import { commands, events, type Result } from "../types/generated";

export { commands, events };

export async function unwrap<T>(resultPromise: Promise<Result<T, string>>): Promise<T> {
  const result = await resultPromise;
  if (result.status === "ok") {
    return result.data;
  }

  throw result.error;
}
