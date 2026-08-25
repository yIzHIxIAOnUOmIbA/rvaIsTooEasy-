import type { DiffResultDto } from "./types";

// Switching views (diff -> settings/symbols -> diff) destroys and rebuilds DiffView.
// A module-level cache preserves the diff result so progress and results aren't lost on rebuild.
export interface DiffCache {
  result: DiffResultDto;
  elapsed: number;
  activeIndex: number;
  // File paths captured at diff time; verified when a view switch restores the cache.
  // If the user changes the file (patch page / drag-drop / input), a path mismatch drops the old cache and re-diffs automatically.
  pathA: string;
  pathB: string;
}

let cache: DiffCache | null = null;

export function cacheDiff(v: DiffCache) {
  cache = v;
}

export function getCachedDiff(): DiffCache | null {
  return cache;
}
