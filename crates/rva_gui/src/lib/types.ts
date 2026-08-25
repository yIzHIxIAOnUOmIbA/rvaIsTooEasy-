export interface FileInfoDto {
  path: string;
  size: number;
  format: string;
  arch: string;
  entry_point: number | null;
}

export interface SummaryDto {
  added: number;
  removed: number;
  modified: number;
  total_bytes: number;
}

export type ChangeKind = "Added" | "Removed" | "Modified";

export interface DiffEntryDto {
  offset: number;
  length: number;
  change: ChangeKind;
  old_start: number | null;
  old_end: number | null;
  new_start: number | null;
  new_end: number | null;
}

export interface DiffResultDto {
  file_a: FileInfoDto;
  file_b: FileInfoDto;
  summary: SummaryDto;
  entries: DiffEntryDto[];
  /** Actual total diff count (may exceed entries.length when truncated) */
  entries_total: number;
  /** Whether entries were truncated for performance protection */
  entries_truncated: boolean;
  /** Strategy that actually took effect (may be overridden by fallback logic) */
  strategy_used: string;
  /** Whether the strategy auto-fell back (e.g. sliding diverged -> chunked) */
  strategy_fallback: boolean;
}

/** Highlight ranges used for coloring within a panel. */
export interface HighlightRange {
  start: number;
  end: number;
  kind: ChangeKind;
}

/** Derive the A/B panel highlight ranges from a DiffResultDto. */
export function buildRanges(entries: DiffEntryDto[], side: "A" | "B"): HighlightRange[] {
  const out: HighlightRange[] = [];
  for (const e of entries) {
    if (e.change === "Modified") {
      // In-place modification: panel A marks the old range red (Removed), panel B the new range green (Added).
      // Split into red/green only at the display layer; the diff list stays a single entry, so it never blows up.
      if (side === "A" && e.old_start !== null && e.old_end !== null) {
        out.push({ start: e.old_start, end: e.old_end, kind: "Removed" });
      } else if (side === "B" && e.new_start !== null && e.new_end !== null) {
        out.push({ start: e.new_start, end: e.new_end, kind: "Added" });
      }
      continue;
    }
    const s = side === "A" ? e.old_start : e.new_start;
    const t = side === "A" ? e.old_end : e.new_end;
    if (s !== null && t !== null) {
      out.push({ start: s, end: t, kind: e.change });
    }
  }
  out.sort((a, b) => a.start - b.start);
  return out;
}

/** Symbol table entry (address -> function name). */
export interface SymbolDto {
  addr: number;
  name: string;
  size: number | null;
}

/** Recursive batch-comparison tree node. */
export interface BatchNodeDto {
  path_a: string | null;
  path_b: string | null;
  status: string;
  diff_count: number | null;
  children: BatchNodeDto[];
}
