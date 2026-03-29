import { searchText } from "../api/meetings";
import type { SearchResult } from "../types";

export interface DebouncedSearch {
  readonly results: SearchResult[];
  readonly isSearching: boolean;
  update(query: string): void;
}

/**
 * Creates a debounced FTS5 search helper with reactive state.
 * Returns a Svelte 5 reactive object — results and isSearching update automatically.
 */
export function createDebouncedSearch(debounceMs = 250, limit = 20): DebouncedSearch {
  let results = $state<SearchResult[]>([]);
  let isSearching = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function perform(query: string) {
    if (!query.trim()) {
      results = [];
      isSearching = false;
      return;
    }
    isSearching = true;
    try {
      results = await searchText(query.trim(), limit);
    } catch {
      // FTS5 match errors on special chars — fall back gracefully
      results = [];
    } finally {
      isSearching = false;
    }
  }

  function update(query: string) {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void perform(query), debounceMs);
  }

  return {
    get results() { return results; },
    get isSearching() { return isSearching; },
    update,
  };
}

/** Filter search results by source type. */
export function filterResultsByType(results: SearchResult[], sourceType: string): SearchResult[] {
  return results.filter((r) => r.source_type === sourceType);
}

/** Find the FTS5 snippet for a given source in search results. */
export function findSnippet(
  searchResults: SearchResult[],
  sourceType: string,
  sourceId: string,
): string | null {
  const result = searchResults.find(
    (r) => r.source_type === sourceType && r.source_id === sourceId,
  );
  return result?.snippet ?? null;
}

/** Get the set of matched IDs for a given source type from search results. */
export function matchedIdsForType(results: SearchResult[], sourceType: string): Set<string> {
  return new Set(
    results
      .filter((r) => r.source_type === sourceType)
      .map((r) => r.source_id),
  );
}
