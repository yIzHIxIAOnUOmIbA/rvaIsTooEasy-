<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open, save } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, untrack } from "svelte";
  import HexPanel from "./HexPanel.svelte";
  import { buildRanges, type ChangeKind, type DiffEntryDto, type DiffResultDto } from "./types";
  import { settings, setSetting, formatBytes, type CopyFormat } from "./settings.svelte";
  import { cacheDiff, getCachedDiff } from "./diffStore";
  import { fade } from "svelte/transition";

  let {
    pathA = $bindable(),
    pathB = $bindable(),
    onopensettings = () => {},
  }: { pathA: string; pathB: string; onopensettings?: () => void } = $props();

  let result = $state.raw<DiffResultDto | null>(null);
  let loading = $state(false);
  let elapsed = $state(0);
  // The first-screen chunk is preloaded by the parent component in the compare-button (user gesture) context and passed in directly, bypassing the issue of invoke responses getting stuck inside $effect
  let initialA = $state<Uint8Array | null>(null);
  let initialB = $state<Uint8Array | null>(null);
  const CHUNK_SIZE_HP = 65536;
  let progress = $state(0);
  let shownProgress = $state(0);
  let progressRaf = 0;
  // Timeout handle backing up first-line rendering: if HexPanel's first-line signal never arrives (e.g. read error), force-finish to avoid the progress bar getting stuck
  let firstLineSafety: ReturnType<typeof setTimeout> | undefined;
  // Progress easing: the displayed value smoothly approaches the target, avoiding the bar jumping to 100 instantly when the Rust side fires an event.
  // Key fix: read/write shownProgress inside untrack -- otherwise the effect treats the shownProgress it wrote
  // as a dependency again, re-entering itself on every progress update.
  $effect(() => {
    const target = progress;
    cancelAnimationFrame(progressRaf);
    const step = () => {
      const d = untrack(() => target - shownProgress);
      if (Math.abs(d) < 0.5) {
        untrack(() => (shownProgress = target));
        return;
      }
      untrack(() => (shownProgress += d * 0.18));
      progressRaf = requestAnimationFrame(step);
    };
    progressRaf = requestAnimationFrame(step);
  });
  let error = $state("");
  let dirty = $state(false);

  // Task 4: diff strategy is wired to the global settings store (the settings page's "default strategy" takes effect immediately; one-way data flow is written back via setSetting)
  const strategy = $derived(settings.diffStrategy);
  let alignMode = $state<"byte" | "instruction" | "function">("byte");

  // Single k: the shared first-row pixel position of both A/B panels. Scrolling either panel updates k; both render with k as their first row,
  // so as long as the variable is consistent, scrolling is guaranteed to stay in sync by construction.
  let topRow = $state(0);
  let activeIndex = $state(-1);
  // Jump-target pulse hint: each increment of key makes both HexPanels play a double-flash animation (same color as the selected row)
  let pulse = $state<{ row: number; key: number } | null>(null);
  let pulseKey = 0;
  // Diagnostics: the most recent key capture, used to diagnose "arrow keys not working"
  let dbgKey = $state("");

  let panelRatio = $state(0.5);
  type UndoEntry = { side: "A" | "B"; path: string; offset: number; oldByte: number; newByte: number };
  type SearchHit = { side: "A" | "B"; offset: number };
  let undoStack = $state<UndoEntry[]>([]);
  let redoStack = $state<UndoEntry[]>([]);
  let gotoInput: HTMLInputElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();
  let refreshA = $state(0);
  let refreshB = $state(0);

  let gotoValue = $state("");
  let searchValue = $state("");
  let searchSide = $state<"A" | "B" | "both">("A");
  let searchMatches = $state<SearchHit[]>([]);
  let searchIndex = $state(-1);
  let searchLen = $state(0);

  // When searching both sides, hits are split by their owning side; the current-hit index on each side is computed separately so the two sides do not flash at the same time.
  // Consolidation: a side-parameterized helper derives the A/B values, eliminating symmetric duplication (dedup at the same level).
  const hitsForSide = (side: "A" | "B") =>
    (searchSide === side || searchSide === "both") && searchMatches.length > 0
      ? searchMatches.filter((m) => m.side === side).map((m) => ({ offset: m.offset, len: searchLen }))
      : null;
  const searchHitsA = $derived(hitsForSide("A"));
  const searchHitsB = $derived(hitsForSide("B"));
  // Side owning the current hit (only used to decide the highlight target when both sides are searched)
  const searchActiveSide = $derived(
    searchIndex >= 0 && searchIndex < searchMatches.length ? searchMatches[searchIndex].side : null,
  );
  // Computes the index of the "current hit" within one side; returns -1 if not on that side or out of range
  function activeIndexOnSide(side: "A" | "B"): number {
    if (searchActiveSide !== side) return -1;
    let cnt = 0;
    for (let i = 0; i <= searchIndex; i++) {
      if (searchMatches[i].side === side) {
        if (i === searchIndex) return cnt;
        cnt++;
      }
    }
    return -1;
  }
  const searchActiveA = $derived.by(() => activeIndexOnSide("A"));
  const searchActiveB = $derived.by(() => activeIndexOnSide("B"));
  let navFilter = $state<ChangeKind | null>(null);
  let navCloseTimer: ReturnType<typeof setTimeout> | undefined;
  // Task 4: hover-delay timer for the diff badge (settings page "hover delay")
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;
  // The HUD toggle has been migrated to the global settings store (settings.showHud, key rva_show_hud)
  let settingsOpen = $state(false);

  // -- Task F: diff-pop entry expanded details (unified across the three types) --
  // Expansion state: records the globalIndex of the currently expanded entry; null means all collapsed.
  // The engine does not carry byte content, so it is read on demand via read_bytes for the old/new ranges.
  let expandedIdx = $state<number | null>(null);
  let expandedBytes = $state<{ old: number[]; new: number[] } | null>(null);
  let loadingBytes = $state(false);

  // -- Task E: double-click the line number to select the whole row (linked across both panels) --
  // Records the selected whole-row range of the initiating side; the other panel renders a dimmed highlight via the syncSelection prop and scrolls it into view.
  // Clear conditions: the other panel starts a new selection / Esc / click on empty space / file switch (see the spots below).
  let rowSelection = $state<{ start: number; end: number } | null>(null);

  function onSelectRow(sel: { start: number; end: number } | null) {
    rowSelection = sel;
  }

  const ROW_HEIGHT = 20;
  const BYTES_PER_ROW = 16;

  // Max row number shared by both panels (based on the larger file), used as the boundary for arrow-key scrolling
  // Consistent with HexPanel's totalRows semantics: at least 1 row, avoiding boundary distortion when maxRows=0 for an empty file
  const maxRows = $derived(
    result
      ? Math.max(
          1,
          Math.ceil(result.file_a.size / BYTES_PER_ROW),
          Math.ceil(result.file_b.size / BYTES_PER_ROW),
        )
      : 0,
  );

  // Performance guard: rows per scroll window. Large files only load/scroll the 1000 rows inside the window; the rest is reached
  // via "jump to row" or by auto-switching the window when scrolling hits the window boundary.
  const RENDER_CAP_ROWS = 1000;
  // Absolute first row of the current scroll window (shared by both panels)
  let winStart = $state(0);
  // Value of the jump-to-row input (formerly the "⚙ Settings" spot)
  let rowJumpValue = $state("");

  const rangesA = $derived(result ? buildRanges(result.entries, "A") : []);
  const rangesB = $derived(result ? buildRanges(result.entries, "B") : []);

  const activeOffset = $derived(
    result && activeIndex >= 0 && activeIndex < result.entries.length
      ? result.entries[activeIndex].offset
      : -1,
  );

  const activeChange = $derived(
    result && activeIndex >= 0 && activeIndex < result.entries.length
      ? result.entries[activeIndex].change
      : null,
  );

  const changeLabel: Record<ChangeKind, string> = {
    Added: "新增",
    Removed: "移除",
    Modified: "改动",
  };

  /** Readable offset of a diff entry: Added uses the B side, everything else uses the A side. */
  function offsetHex(e: DiffEntryDto): string {
    const off = e.change === "Added" ? e.new_start : e.old_start;
    return "0x" + ((off ?? 0) >>> 0).toString(16).toUpperCase().padStart(8, "0");
  }

  type NavItem = { e: DiffEntryDto; globalIndex: number };
  // Pre-group by type to avoid a full map+filter on every hover (very laggy with tens of thousands of entries)
  const grouped = $derived.by(() => {
    const g: Record<ChangeKind, NavItem[]> = { Added: [], Removed: [], Modified: [] };
    if (result) {
      result.entries.forEach((e, globalIndex) => {
        g[e.change].push({ e, globalIndex });
      });
    }
    return g;
  });
  const NAV_CAP = 1000; // max entries rendered in the popup, preventing DOM freeze with tens of thousands of rows

  // summary.added/removed/modified are all byte counts (aggregated by the backend ReportSummary).
  const kinds = $derived(
    result
      ? [
          { kind: "Added" as ChangeKind, label: "新增", count: result.summary.added, cls: "added" },
          { kind: "Removed" as ChangeKind, label: "移除", count: result.summary.removed, cls: "removed" },
        ]
      : [],
  );

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      try {
        const ta = document.createElement("textarea");
        ta.value = text;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        ta.remove();
      } catch {
        /* ignore */
      }
    }
  }

  // -- P0-5 copy format options: double-click the line number to select a byte range, then copy to clipboard in the chosen format --
  let copySide = $state<"A" | "B">("A");
  let copyNote = $state("");

  async function copySelection() {
    if (!result || !rowSelection) return;
    const path = copySide === "A" ? result.file_a.path : result.file_b.path;
    const size = copySide === "A" ? result.file_a.size : result.file_b.size;
    const start = Math.min(rowSelection.start, size - 1);
    const end = Math.min(rowSelection.end, size - 1);
    if (start > end) {
      copyNote = "无数据";
      return;
    }
    try {
      // Unified copy serialization: toolbar / context menu / HexPanel all share settings.copyFormat (hexsp|hex|dec|sp|diff)
      const data = await invoke<number[]>("read_bytes", { path, offset: start, length: end - start + 1 });
      await copyText(formatBytes(data, settings.copyFormat));
      copyNote = `已复制 ${data.length}B`;
      setTimeout(() => (copyNote = ""), 2000);
    } catch (e) {
      copyNote = "复制失败";
      error = String(e);
    }
  }

  function goEntry(i: number) {
    if (!result) return;
    activeIndex = i;
    jumpTo(i);
    closeNav();
  }

  // -- Task F: byte-detail expansion when switching entries --
  // Unified across the three types: expand whenever either the old or new range has content.
  // Added: only the new range; Removed: only the old range; Modified: both ranges shown side by side.
  async function toggleEntryBytes(item: NavItem) {
    if (!result) return;
    if (expandedIdx === item.globalIndex) {
      expandedIdx = null;
      expandedBytes = null;
      return;
    }
    expandedIdx = item.globalIndex;
    expandedBytes = null;
    loadingBytes = true;
    try {
      const e = item.e;
      const oldBytes: number[] = [];
      const newBytes: number[] = [];
      const lenA = e.old_start !== null && e.old_end !== null ? e.old_end - e.old_start : 0;
      const lenB = e.new_start !== null && e.new_end !== null ? e.new_end - e.new_start : 0;
      // Handles the case where the engine uses length for Added/Removed insertion/deletion size and the range may be empty
      const loadOld = lenA > 0 || (e.change === "Removed" && e.length > 0);
      const loadNew = lenB > 0 || (e.change === "Added" && e.length > 0);
      if (loadOld) {
        const start = e.old_start ?? item.e.offset;
        const length = lenA > 0 ? lenA : e.length;
        const data = await invoke<number[]>("read_bytes", {
          path: result.file_a.path,
          offset: start,
          length,
        });
        oldBytes.push(...data);
      }
      if (loadNew) {
        const start = e.new_start ?? item.e.offset;
        const length = lenB > 0 ? lenB : e.length;
        const data = await invoke<number[]>("read_bytes", {
          path: result.file_b.path,
          offset: start,
          length,
        });
        newBytes.push(...data);
      }
      expandedBytes = { old: oldBytes, new: newBytes };
    } catch (err) {
      console.error("[DiffView] 加载条目字节失败:", err);
      expandedBytes = { old: [], new: [] };
    } finally {
      loadingBytes = false;
    }
  }

  // Hex byte string shown in the expanded area (two characters)
  function byteHex(b: number): string {
    return b.toString(16).padStart(2, "0").toUpperCase();
  }

  // On single click of a row (HexPanel reports the in-row byte offset), match diff entries at "row" granularity:
  // compute the byte range covered by the clicked row [row start, row start + row width); a hit occurs when the
  // entry's old/new range intersects that range (even if the diff starts mid-row), and the entry with the smallest index wins.
  // On a miss (the whole row is not a diff), keep the current selection so progress is not cleared.
  // Both sides are matched: the A-panel offset falls in the old range, the B-panel in the new range; either side hitting is enough.
  function onRowSelect(offset: number) {
    if (!result || result.entries.length === 0) return;
    const entries = result.entries;
    const rowStart = offset - (offset % BYTES_PER_ROW);
    const rowEnd = rowStart + BYTES_PER_ROW - 1;
    const hits = (e: (typeof entries)[number]) =>
      (e.old_start !== null && e.old_end !== null && e.old_start <= rowEnd && e.old_end > rowStart) ||
      (e.new_start !== null && e.new_end !== null && e.new_start <= rowEnd && e.new_end > rowStart);
    let found = -1;
    for (let i = 0; i < entries.length; i++) {
      if (hits(entries[i])) {
        found = i;
        break;
      }
    }
    // On a miss (clicking a non-diff character/row), keep the current progress so the bottom-left counter does not drop back to 0
    if (found >= 0) activeIndex = found;
  }

  function openNavFor(kind: ChangeKind) {
    if (navCloseTimer) {
      clearTimeout(navCloseTimer);
      navCloseTimer = undefined;
    }
    navFilter = kind;
  }
  function scheduleCloseNav() {
    if (navCloseTimer) clearTimeout(navCloseTimer);
    navCloseTimer = setTimeout(() => {
      navFilter = null;
      navCloseTimer = undefined;
    }, 240);
  }
  function closeNav() {
    if (navCloseTimer) {
      clearTimeout(navCloseTimer);
      navCloseTimer = undefined;
    }
    navFilter = null;
  }

  async function browseA() {
    const f = await open({ multiple: true, title: "选择文件 A" });
    applyPicked(f, "A");
  }
  async function browseB() {
    const f = await open({ multiple: true, title: "选择文件 B" });
    applyPicked(f, "B");
  }

  /** Sequential selection: pick A first; if only one was picked at a time, then prompt for B; when both are chosen, compare automatically. */
  async function browseBoth() {
    try {
      const first = await open({ multiple: true, title: "选择文件 A" });
      if (!first) return;
      const files = Array.isArray(first) ? first : [first];
      if (files.length === 0) return;
      pathA = files[0];
      if (files.length >= 2) {
        pathB = files[1];
      } else {
        const second = await open({ multiple: false, title: "选择文件 B" });
        if (!second || Array.isArray(second)) return; // user cancelled
        pathB = second;
      }
      await compare();
    } catch (e) {
      error = String(e);
    }
  }

  function applyPicked(f: string | string[] | null, primary: "A" | "B") {
    if (!f) return;
    const files = Array.isArray(f) ? f : [f];
    if (files.length === 0) return;
    if (files.length >= 2) {
      pathA = files[0];
      pathB = files[1];
    } else if (primary === "A") {
      pathA = files[0];
    } else {
      pathB = files[0];
    }
  }

  async function compare(opts?: { noJump?: boolean }) {
    if (!pathA || !pathB) return;
    loading = true;
    progress = 0;
    shownProgress = 0;
    error = "";
    if (firstLineSafety) { clearTimeout(firstLineSafety); firstLineSafety = undefined; }
    // Task E: clear the whole-row selection when switching/re-comparing files
    rowSelection = null;
    const t0 = performance.now();
    // Fallback: if HexPanel's first-line signal never arrives due to an error, force-finish after at most 3s to avoid the progress bar getting stuck
    firstLineSafety = setTimeout(finishLoading, 3000);
    try {
      const res = await invoke<DiffResultDto>("diff_files", { pathA, pathB, strategy, alignMode });
      elapsed = performance.now() - t0;

      // Key fix: preload the first-screen chunk first, then set result so HexPanel mounts. This way, when HexPanel is first created,
      // initialChunk already has data, so the initialChunk effect can write the cache and render immediately, avoiding the
      // "mount first, get initialChunk later" $effect timing/reactivity-loss problem.
      initialA = null;
      initialB = null;
      // Reset to zero before computing the first-screen chunk: otherwise the topRow left over from the previous scroll
      // misaligns the preloaded chunk index with HexPanel's zeroed first-screen chunk, leaving chunk 0 unloaded and blank.
      topRow = 0;
      const firstVisibleByte = Math.floor(topRow / ROW_HEIGHT) * BYTES_PER_ROW;
      const c = Math.floor(firstVisibleByte / CHUNK_SIZE_HP);
      try {
        const d = await invoke<number[]>("read_bytes", {
          path: pathA,
          offset: c * CHUNK_SIZE_HP,
          length: CHUNK_SIZE_HP,
        });
        initialA = new Uint8Array(d);
      } catch (e) {
        console.error("[DiffView] preloadA failed", e);
      }
      try {
        const d = await invoke<number[]>("read_bytes", {
          path: pathB,
          offset: c * CHUNK_SIZE_HP,
          length: CHUNK_SIZE_HP,
        });
        initialB = new Uint8Array(d);
      } catch (e) {
        console.error("[DiffView] preloadB failed", e);
      }

      // Data is ready: progress goes straight to 100 and finishes immediately. No longer relies on HexPanel's first-line signal,
      // because if that signal never arrives (chunk read error / blockage), the progress bar and the whole UI would freeze permanently.
      result = res;
      progress = 100;
      finishLoading();
      activeIndex = result.entries.length > 0 ? 0 : -1;
      topRow = 0;
      dirty = false;
      if (activeIndex >= 0 && !opts?.noJump) jumpTo(0);
      // Cache the diff result: restored when switching views (settings/symbols) and back, avoiding lost progress and results
      cacheDiff({ result: res, elapsed, activeIndex, pathA, pathB });
      // Trigger the panels to reload their visible area (when the file changes)
      refreshA++;
      refreshB++;
    } catch (e) {
      if (firstLineSafety) { clearTimeout(firstLineSafety); firstLineSafety = undefined; }
      error = String(e);
      result = null;
      loading = false;
    }
  }

  // Called back when HexPanel finishes rendering its first line; the safety fallback also calls it. Guarantees finishing exactly once.
  function finishLoading() {
    if (!loading) return;
    if (firstLineSafety) { clearTimeout(firstLineSafety); firstLineSafety = undefined; }
    loading = false;
    progress = 100;
    shownProgress = 100;
  }

  // User scrolls either panel: update the shared k (topRow); both render with k as their first row, so scrolling stays in sync.
  // No longer moves the "current entry" to the scroll position -- explicit user selection (click/jump) keeps top priority.
  // The scrollbar is full-range (covers the whole file); the window only decides the render range: crossing the window
  // boundary during a scroll is followed by ensureWindow, so dragging is continuous without snapping back, and fast
  // wheel / drag-to-bottom never produces a "0..N row loop".
  function onScroll(row: number) {
    topRow = row;
    ensureWindow(Math.floor(row / ROW_HEIGHT));
  }

  // Ensure the absolute row number falls inside the scroll window (switch windows otherwise, keeping a little overlap to avoid jumps).
  // Every jump (entry/offset/search/row) must first ensure the window contains the target row,
  // otherwise the target cannot be scrolled to in window mode, appearing as "clicking an entry does not jump".
  function ensureWindow(rowAbs: number) {
    const cap = RENDER_CAP_ROWS;
    if (rowAbs < winStart || rowAbs >= winStart + cap) {
      const maxStart = Math.max(0, maxRows - cap);
      winStart = Math.max(0, Math.min(rowAbs - 4, maxStart));
    }
  }
  // When HexPanel scrolls to the window boundary, switch the window on demand (performance guard: only the current window is loaded; scrolling to the bottom advances automatically).
  // Only advance the render window, do not move the absolute position topRow -- the scrollbar is full-range, so window switching must not yank the user back to the window top.
  function shiftWindow(dir: "prev" | "next") {
    const cap = RENDER_CAP_ROWS;
    if (dir === "next") {
      winStart = Math.min(Math.max(0, maxRows - cap), winStart + cap - 100);
    } else {
      winStart = Math.max(0, winStart - cap + 100);
    }
  }

  // Sync the cache when the selected entry changes: switching pages (settings/symbols) and back restores the real selection,
  // instead of returning to the first diff at the moment the comparison finished (the old implementation saved activeIndex only once at completion).
  $effect(() => {
    if (result) cacheDiff({ result, elapsed, activeIndex, pathA, pathB });
  });

  function jumpTo(i: number) {
    if (!result) return;
    const e = result.entries[i];
    // Single k: the entry's anchor offset determines the first row, and both A/B sides scroll to the same k.
    // Added's offset lives on the B side (A-panel coordinates are invalid), Removed's on the A side;
    // unified by taking old_start/new_start per type, fixing "only Modify jumps, the rest stay put".
    const off = e.change === "Added" ? (e.new_start ?? e.offset) : (e.old_start ?? e.offset);
    topRow = Math.floor((off ?? 0) / BYTES_PER_ROW) * ROW_HEIGHT;
    ensureWindow(Math.floor(topRow / ROW_HEIGHT));
    // Jump-target pulse hint: both panels flash the target row twice (same color as the selected row), clearly showing where the jump landed
    pulse = { row: Math.floor(topRow / ROW_HEIGHT), key: ++pulseKey };
  }

  // After a single-byte edit, automatically re-compare a small scope: 250ms debounce, preserving scroll position and selection
  // across the recompute, avoiding "the whole page jumps back to the top after editing one byte". Part of the 0.2.1 fixes.
  let rediffTimer: ReturnType<typeof setTimeout> | undefined;
  function scheduleRediff() {
    if (rediffTimer) clearTimeout(rediffTimer);
    rediffTimer = setTimeout(() => {
      const prevTop = topRow;
      const prevIdx = activeIndex;
      void compare({ noJump: true }).then(() => restoreAfterRecompute(prevTop, prevIdx));
    }, 250);
  }
  // Restore the view after recompute: recover scroll offset and selected index so editing feels continuous
  function restoreAfterRecompute(prevTop: number, prevIdx: number) {
    topRow = prevTop;
    if (prevIdx >= 0 && result && prevIdx < result.entries.length) activeIndex = prevIdx;
  }
  async function recordEditA(info: { offset: number; oldByte: number; newByte: number }) {
    undoStack.push({ side: "A", path: pathA, offset: info.offset, oldByte: info.oldByte, newByte: info.newByte });
    redoStack = [];
    scheduleRediff();
  }
  async function recordEditB(info: { offset: number; oldByte: number; newByte: number }) {
    undoStack.push({ side: "B", path: pathB, offset: info.offset, oldByte: info.oldByte, newByte: info.newByte });
    redoStack = [];
    scheduleRediff();
  }

  async function undo() {
    const entry = undoStack.pop();
    if (!entry) return;
    try {
      await invoke("write_bytes", { path: entry.path, offset: entry.offset, data: [entry.oldByte] });
      redoStack.push(entry);
      if (entry.side === "A") refreshA++;
      else refreshB++;
    } catch (e) {
      error = String(e);
    }
  }

  async function redo() {
    const entry = redoStack.pop();
    if (!entry) return;
    try {
      await invoke("write_bytes", { path: entry.path, offset: entry.offset, data: [entry.newByte] });
      undoStack.push(entry);
      if (entry.side === "A") refreshA++;
      else refreshB++;
    } catch (e) {
      error = String(e);
    }
  }

  async function saveAs(side: "A" | "B") {
    const src = side === "A" ? pathA : pathB;
    if (!src) return;
    const base = src.split(/[\\/]/).pop() || "file";
    const dot = base.lastIndexOf(".");
    const def = dot > 0 ? base.slice(0, dot) + "_copy" + base.slice(dot) : base + "_copy.bin";
    const dst = await save({ title: `另存 ${side}`, defaultPath: def });
    if (!dst) return;
    try {
      await invoke("copy_file", { src, dst });
    } catch (e) {
      error = String(e);
    }
  }

  function gotoOffset() {
    if (!result) return;
    const v = gotoValue.trim().replace(/^0x/i, "").replace(/[\s_]/g, "");
    if (!/^[0-9a-fA-F]+$/.test(v)) {
      error = "无效偏移";
      return;
    }
    const off = parseInt(v, 16);
    topRow = Math.floor(off / BYTES_PER_ROW) * ROW_HEIGHT;
    ensureWindow(Math.floor(topRow / ROW_HEIGHT));
  }

  function parseHexPattern(s: string): number[] | null {
    const clean = s.trim().replace(/0x/gi, "").replace(/[\s,:_\-]/g, "");
    if (clean.length === 0 || clean.length % 2 !== 0 || !/^[0-9a-fA-F]+$/.test(clean)) return null;
    const out: number[] = [];
    for (let i = 0; i < clean.length; i += 2) out.push(parseInt(clean.slice(i, i + 2), 16));
    return out;
  }

  async function doSearch() {
    if (!result) return;
    const pat = parseHexPattern(searchValue);
    if (!pat) {
      error = "无效字节序列（例如 FF 15 90）";
      return;
    }
    let hits: SearchHit[] = [];
    try {
      if (searchSide === "A") {
        const offs = await invoke<number[]>("search_bytes", { path: pathA, pattern: pat, maxMatches: 100000 });
        hits = offs.map((o) => ({ side: "A", offset: o }));
      } else if (searchSide === "B") {
        const offs = await invoke<number[]>("search_bytes", { path: pathB, pattern: pat, maxMatches: 100000 });
        hits = offs.map((o) => ({ side: "B", offset: o }));
      } else {
        const [a, b] = await Promise.all([
          invoke<number[]>("search_bytes", { path: pathA, pattern: pat, maxMatches: 100000 }),
          invoke<number[]>("search_bytes", { path: pathB, pattern: pat, maxMatches: 100000 }),
        ]);
        hits = [
          ...a.map((o) => ({ side: "A" as const, offset: o })),
          ...b.map((o) => ({ side: "B" as const, offset: o })),
        ].sort((x, y) => x.offset - y.offset);
      }
      searchLen = pat.length;
      searchMatches = hits;
      searchIndex = hits.length > 0 ? 0 : -1;
      if (hits.length > 0) gotoMatch(hits[0].offset);
    } catch (e) {
      error = String(e);
      searchMatches = [];
      searchIndex = -1;
    }
  }

  function gotoMatch(off: number) {
    topRow = Math.floor(off / BYTES_PER_ROW) * ROW_HEIGHT;
    ensureWindow(Math.floor(topRow / ROW_HEIGHT));
    pulse = { row: Math.floor(topRow / ROW_HEIGHT), key: ++pulseKey };
  }
  // Jump by row: enter a 1-based row number, auto-switch the window and scroll to that row
  // (performance guard: content outside the window is not rendered; enter via the jump feature)
  function rowJump() {
    if (!result) return;
    const n = parseInt(rowJumpValue, 10);
    if (Number.isNaN(n) || n < 1) {
      error = "无效行号";
      return;
    }
    const rowAbs = Math.min(n - 1, maxRows - 1);
    ensureWindow(rowAbs);
    topRow = rowAbs * ROW_HEIGHT;
    pulse = { row: rowAbs, key: ++pulseKey };
  }
  function nextMatch() {
    if (searchMatches.length === 0) return;
    searchIndex = (searchIndex + 1) % searchMatches.length;
    gotoMatch(searchMatches[searchIndex].offset);
  }
  function prevMatch() {
    if (searchMatches.length === 0) return;
    searchIndex = (searchIndex - 1 + searchMatches.length) % searchMatches.length;
    gotoMatch(searchMatches[searchIndex].offset);
  }

  // Core paging logic: shared by the capture-phase listener registered in onMount and HexPanel's container fallback.
  // Returning true means the key has been consumed (preventDefault + stopPropagation already applied).
  function handleNavKey(e: KeyboardEvent): boolean {
    if (e.defaultPrevented) return false; // prevent re-entry from double registration
    // Task A: global Ctrl+O to open files (handled first in the capture phase; works even when an input is focused)
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "o") {
      e.preventDefault();
      e.stopPropagation();
      void browseBoth();
      return true;
    }
    // P0-5 shortcuts: Ctrl+G jump to offset (focuses the jump input), Ctrl+F find bytes (focuses the search input)
    // Placed before the INPUT interception so they work globally; when focus is already in an input, the browser's native behavior takes over.
    if (e.ctrlKey || e.metaKey) {
      const k = e.key.toLowerCase();
      if (k === "g") {
        e.preventDefault();
        e.stopPropagation();
        gotoInput?.focus();
        gotoInput?.select();
        dbgKey = "Ctrl+G → goto";
        return true;
      }
      if (k === "f") {
        e.preventDefault();
        e.stopPropagation();
        searchInput?.focus();
        searchInput?.select();
        dbgKey = "Ctrl+F → search";
        return true;
      }
      if (k === "z") {
        e.preventDefault();
        e.stopPropagation();
        void undo();
        dbgKey = "Ctrl+Z → undo";
        return true;
      }
      if (k === "y") {
        e.preventDefault();
        e.stopPropagation();
        void redo();
        dbgKey = "Ctrl+Y → redo";
        return true;
      }
    }
    const t = e.target as HTMLElement | null;
    const tag = t ? t.tagName : "null";
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) {
      dbgKey = `${e.key} blocked(INPUT ${tag})`;
      return false;
    }
    if (!result) {
      dbgKey = `${e.key} blocked(no-result)`;
      return false;
    }
    let step = 0;
    if (e.key === "ArrowDown") step = 1;
    else if (e.key === "ArrowUp") step = -1;
    else if (e.key === "PageDown") step = 40;
    else if (e.key === "PageUp") step = -40;
    else if (e.key === "Home" || e.key === "End") step = -999;
    if (step === 0) {
      dbgKey = `${e.key} ignored(step0)`;
      return false;
    }
    e.preventDefault();
    e.stopPropagation();
    const maxScroll = Math.max(0, (maxRows - 1) * ROW_HEIGHT);
    const before = topRow;
    if (e.key === "Home") {
      topRow = 0;
    } else if (e.key === "End") {
      topRow = maxScroll;
    } else {
      topRow = Math.max(0, Math.min(maxScroll, topRow + step * ROW_HEIGHT));
    }
    dbgKey = `${e.key} t=${tag} k=${before}->${topRow} max=${maxScroll}`;
    return true;
  }

  function onDividerDown(e: MouseEvent) {
    e.preventDefault();
    const move = (ev: MouseEvent) => {
      const el = document.querySelector(".panels") as HTMLElement | null;
      if (!el) return;
      const r = el.getBoundingClientRect();
      panelRatio = Math.min(0.85, Math.max(0.15, (ev.clientX - r.left) / r.width));
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;

    // When the view switches (compare -> settings/symbols -> compare), the component is destroyed and rebuilt: restore the diff result
    // from the module cache so existing progress and results are not lost. If the files are unchanged, return to the last result directly;
    // if a path changed (e.g. a new file was picked on the patch page), drop the old cache and re-compare automatically.
    const saved = getCachedDiff();
    const pathsMatch = !!saved && saved.pathA === pathA && saved.pathB === pathB;
    if (saved && saved.result && pathsMatch) {
      result = saved.result;
      elapsed = saved.elapsed;
      progress = 100;
      shownProgress = 100;
      activeIndex = Math.min(saved.activeIndex, saved.result.entries.length - 1);
      // Restore the selection: the viewport returns to the row of the last selected entry (with a pulse hint), instead of staying at the top of the file
      if (activeIndex >= 0 && activeIndex < saved.result.entries.length) {
        jumpTo(activeIndex);
      } else {
        topRow = 0;
      }
      dirty = false;
    } else if (pathA && pathB) {
      // A path changed (e.g. a new file picked on the patch page) or never compared before: run the comparison automatically, no need for the user to click "Compare" again.
      // Also applies on first entry when both A/B paths are ready (synced from the patch page).
      compare();
    }

    // Keyboard paging: uses a document capture-phase listener (instead of window bubbling), so arrow keys / PageUp / PageDown
    // are received no matter where focus is (body, buttons, panels), independent of whether focus is on the panel container.
    // The capture phase runs before the target element; after handling, stopPropagation prevents the event from continuing down,
    // avoiding the panel container's default scrolling or input behaviors.
    // Note: must be registered before getCurrentWebview(), so a drag-listener init exception cannot block keyboard registration.
    // Double registration: both window and document carry capture-phase listeners. In some WebView/focus scenarios one of them
    // misses events, so the double insurance keeps arrow keys / paging working. The capture phase precedes the target element,
    // and stopPropagation does not disturb inputs / default scrolling.
    const onKeyNav = (e: KeyboardEvent) => {
      handleNavKey(e);
    };
    document.addEventListener("keydown", onKeyNav, true);
    window.addEventListener("keydown", onKeyNav, true);

    try {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type !== "drop") return;
          const files = event.payload.paths;
          if (files.length >= 2) {
            pathA = files[0];
            pathB = files[1];
          } else if (files.length === 1) {
            if (!pathA) pathA = files[0];
            else pathB = files[0];
          }
        })
        .then((fn) => (unlisten = fn));
    } catch {
      // A drag-listener init failure does not affect core features (keyboard/scroll/compare).
    }

    listen<number>("diff-progress", (e) => {
      // Cap at 96%: after the compare algorithm finishes there is still unknown serialization/transfer time that cannot be
      // measured precisely, so it stops at 96 and tops up to 100 once HexPanel actually renders its first line
      progress = Math.min(e.payload, 96);
    }).then((fn) => (unlistenProgress = fn));

    return () => {
      unlisten?.();
      unlistenProgress?.();
      document.removeEventListener("keydown", onKeyNav, true);
      window.removeEventListener("keydown", onKeyNav, true);
    };
  });

  function prev() {
    if (!result || result.entries.length === 0) return;
    activeIndex = activeIndex <= 0 ? result.entries.length - 1 : activeIndex - 1;
    jumpTo(activeIndex);
  }
  function next() {
    if (!result || result.entries.length === 0) return;
    activeIndex = activeIndex >= result.entries.length - 1 ? 0 : activeIndex + 1;
    jumpTo(activeIndex);
  }

  function fmtSize(n: number): string {
    if (n >= 1 << 30) return (n / (1 << 30)).toFixed(2) + " GiB";
    if (n >= 1 << 20) return (n / (1 << 20)).toFixed(2) + " MiB";
    if (n >= 1 << 10) return (n / (1 << 10)).toFixed(1) + " KiB";
    return n + " B";
  }
</script>

<div class="diff">
  <div class="toolbar">
    <button
      class="btn primary"
      onclick={browseBoth}
      title="连续选择文件：先选 A，再选 B，选满自动比对"
    >连续选择</button>
    <div class="path-row">
      <input class="path-input a" type="text" placeholder="文件 A 路径" bind:value={pathA} />
      <button class="btn" onclick={browseA}>更换</button>
    </div>
    <select class="sel" value={strategy} title="比对策略"
      onchange={(e) => setSetting("diffStrategy", e.currentTarget.value as "chunked" | "sliding" | "structural")}>
      <option value="chunked">分块哈希</option>
      <option value="sliding">滑动窗口</option>
      <option value="structural">函数级匹配</option>
    </select>
    <select class="sel" bind:value={alignMode} title="对齐级别">
      <option value="byte">字节对齐</option>
      <option value="instruction">指令对齐</option>
      <option value="function">函数对齐</option>
    </select>
    <button class="btn primary" onclick={compare} disabled={loading || !pathA || !pathB}>
      {loading
        ? shownProgress >= 90
          ? "整理结果中…"
          : `比对中 ${Math.round(shownProgress)}%`
        : "比对"}
    </button>
    <div class="path-row">
      <input class="path-input b" type="text" placeholder="文件 B 路径" bind:value={pathB} />
      <button class="btn" onclick={browseB}>更换</button>
    </div>
  </div>

  {#if loading}
    <div class="progress-track" aria-hidden="true">
      <div class="progress-bar" style={`width: ${shownProgress}%`}></div>
    </div>
  {/if}

  {#if error}
    <div class="error">{error}</div>
  {/if}

  {#if result}
    {#if dirty}
      <div class="dirty">
        文件已修改，比对结果已过时。
        <button class="btn" onclick={compare} disabled={loading}>重新比对</button>
      </div>
    {/if}

    <div class="fileinfo">
      <div class="finfo">
        <span class="fname a">A</span>
        <span>{result.file_a.format}</span>
        <span>{result.file_a.arch}</span>
        <span>{fmtSize(result.file_a.size)}</span>
      </div>
      <div class="badges">
        {#each kinds as k (k.kind)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="badge-wrap"
            class:open={navFilter === k.kind}
            onmouseenter={() => {
              if (hoverTimer) clearTimeout(hoverTimer);
              // Re-focused (including mouseenter bubbling through the pop): cancel the pending collapse timer
              if (navCloseTimer) {
                clearTimeout(navCloseTimer);
                navCloseTimer = undefined;
              }
              hoverTimer = setTimeout(() => openNavFor(k.kind), settings.badgeHoverDelay);
            }}
            onmouseleave={() => {
              if (hoverTimer) clearTimeout(hoverTimer);
              scheduleCloseNav();
            }}
          >
            <div class="badge {k.cls}">
              <span class="badge-dot"></span>
              <span class="badge-label">{k.label}</span>
              <span class="badge-num">{k.count}</span>
              <svg class="chev" viewBox="0 0 16 16" width="9" height="9" aria-hidden="true">
                <path
                  d="M4 6l4 4 4-4"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.6"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
            </div>
            {#if navFilter === k.kind}
              <!-- Task 4: collapse only after 3s of idle once the mouse leaves the pop; fade provides smooth enter/collapse animation (never touches transform, avoiding a conflict with translateX(-50%)) -->
              <div
                class="diff-pop"
                transition:fade={{ duration: 180 }}
                onmouseleave={() => scheduleCloseNav()}
              >
                <div class="pop-head">{k.label} · 点击条目展开/收起字节详情 · ⧉ 复制偏移{grouped[k.kind].length > NAV_CAP ? `（仅显示前 ${NAV_CAP} 条，其余用下方按钮导航）` : ""}</div>
                <div class="pop-list">
                  {#if grouped[k.kind].length === 0}
                    <div class="pop-empty">该类型暂无差异条目</div>
                  {:else}
                  {#each grouped[k.kind].slice(0, NAV_CAP) as item, j (item.globalIndex)}
                    <div
                      class={`pop-entry ${item.e.change.toLowerCase()}`}
                      class:expanded={expandedIdx === item.globalIndex}
                      class:active={item.globalIndex === activeIndex}
                    >
                      <button
                        class="pop-item"
                        onclick={() => {
                          goEntry(item.globalIndex);
                          toggleEntryBytes(item);
                        }}
                      >
                        <span class="pop-idx">{j + 1}</span>
                        <span class="pop-off">{offsetHex(item.e)}</span>
                        <span class="pop-len">{item.e.length} B</span>
                        <svg class="pop-arrow" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
                          <path
                            d="M6 4l4 4-4 4"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="1.6"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                          />
                        </svg>
                        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <span
                          class="pop-copy"
                          title="复制偏移"
                          onclick={(ev) => {
                            ev.stopPropagation();
                            copyText(offsetHex(item.e));
                          }}
                        >⧉</span>
                      </button>
                      {#if expandedIdx === item.globalIndex}
                        <div class="pop-detail" class:loading={loadingBytes}>
                          {#if loadingBytes}
                            <div class="pop-detail-loading">加载字节…</div>
                          {:else if expandedBytes}
                            {#if item.e.change === "Modified"}
                              <div class="byte-cols">
                                <div class="byte-col old">
                                  <div class="byte-col-title">原始字节</div>
                                  <div class="byte-grid">
                                    {#each expandedBytes.old as b, bi (bi)}
                                      <span
                                        class="byte-cell"
                                        class:diff={b !== expandedBytes.new[bi]}
                                        class:same={b === expandedBytes.new[bi]}
                                      >{byteHex(b)}</span>
                                    {/each}
                                  </div>
                                </div>
                                <div class="byte-col new">
                                  <div class="byte-col-title">替换字节</div>
                                  <div class="byte-grid">
                                    {#each expandedBytes.new as b, bi (bi)}
                                      <span
                                        class="byte-cell"
                                        class:diff={b !== expandedBytes.old[bi]}
                                        class:same={b === expandedBytes.old[bi]}
                                      >{byteHex(b)}</span>
                                    {/each}
                                  </div>
                                </div>
                              </div>
                            {:else if item.e.change === "Added"}
                              <div class="byte-cols">
                                <div class="byte-col none">
                                  <div class="byte-col-title">原始字节</div>
                                  <div class="byte-col-hint">无原始字节</div>
                                </div>
                                <div class="byte-col new">
                                  <div class="byte-col-title">插入字节</div>
                                  <div class="byte-grid">
                                    {#each expandedBytes.new as b, bi (bi)}
                                      <span class="byte-cell">{byteHex(b)}</span>
                                    {/each}
                                  </div>
                                </div>
                              </div>
                            {:else}
                              <div class="byte-cols">
                                <div class="byte-col old">
                                  <div class="byte-col-title">删除字节</div>
                                  <div class="byte-grid">
                                    {#each expandedBytes.old as b, bi (bi)}
                                      <span class="byte-cell">{byteHex(b)}</span>
                                    {/each}
                                  </div>
                                </div>
                                <div class="byte-col none">
                                  <div class="byte-col-title">替换字节</div>
                                  <div class="byte-col-hint">无替换字节</div>
                                </div>
                              </div>
                            {/if}
                          {/if}
                        </div>
                      {/if}
                    </div>
                  {/each}
                  {/if}
                </div>
              </div>
            {/if}
          </div>
        {/each}
      </div>
      <div class="finfo right">
        <span class="fname b">B</span>
        <span>{result.file_b.format}</span>
        <span>{result.file_b.arch}</span>
        <span>{fmtSize(result.file_b.size)}</span>
      </div>
    </div>

    <div class="toolrow">
      <button class="btn" onclick={undo} disabled={undoStack.length === 0} title="撤销（Ctrl+Z）">↶ 撤销</button>
      <button class="btn" onclick={redo} disabled={redoStack.length === 0} title="重做（Ctrl+Y）">↷ 重做</button>
      <button class="btn" onclick={() => saveAs("A")} disabled={!pathA}>另存 A</button>
      <button class="btn" onclick={() => saveAs("B")} disabled={!pathB}>另存 B</button>
      <span class="sep"></span>
      <span class="diff-nav">
        <button class="btn" onclick={prev} disabled={!result || result.entries.length === 0} title="上一个差异">↑</button>
        <button class="btn" onclick={next} disabled={!result || result.entries.length === 0} title="下一个差异">↓</button>
        <span class="diff-n">{result ? `${activeIndex + 1}/${result.entries.length}` : "0/0"}</span>
      </span>
      <span class="sep"></span>
      <input class="mini" type="text" placeholder="偏移(hex)" bind:value={gotoValue} bind:this={gotoInput} onkeydown={(e) => e.key === "Enter" && gotoOffset()} />
      <button class="btn" onclick={gotoOffset}>跳转</button>
      <span class="sep"></span>
      <input class="mini search" type="text" placeholder="搜索字节 FF 15" bind:value={searchValue} bind:this={searchInput} onkeydown={(e) => e.key === "Enter" && doSearch()} />
      <select class="sel" bind:value={searchSide} title="搜索范围：A / B / 两侧">
        <option value="A">A</option>
        <option value="B">B</option>
        <option value="both">两侧</option>
      </select>
      <button class="btn" onclick={doSearch}>搜索</button>
      <button class="btn" onclick={prevMatch} disabled={searchMatches.length === 0}>↑</button>
      <button class="btn" onclick={nextMatch} disabled={searchMatches.length === 0}>↓</button>
      <span class="search-n">{searchMatches.length > 0 ? `${searchIndex + 1}/${searchMatches.length}` : ""}</span>
      <span class="sep"></span>
      <select class="sel" bind:value={copySide} title="复制源文件">
        <option value="A">A</option>
        <option value="B">B</option>
      </select>
      <select class="sel" value={settings.copyFormat} onchange={(e) => setSetting("copyFormat", (e.currentTarget as HTMLSelectElement).value as CopyFormat)} title="复制格式">
        <option value="hexsp">Hex 空格</option>
        <option value="hex">Hex 紧凑</option>
        <option value="carr">C 数组</option>
        <option value="rarr">Rust 数组</option>
        <option value="py">Python bytes</option>
        <option value="ascii">ASCII</option>
      </select>
      <button class="btn" onclick={copySelection} disabled={!result || !rowSelection} title="双击行号选中范围后复制">复制</button>
      <span class="search-n">{copyNote}</span>
      <span class="sep"></span>
      <input class="mini rowjump" type="text" placeholder="行号跳转" bind:value={rowJumpValue} onkeydown={(e) => e.key === "Enter" && rowJump()} />
      <button class="btn" onclick={rowJump} title="跳到指定行（性能保护：窗口外内容需跳转进入）">跳行</button>
    </div>

    <div class="panels">
      <div class="panel-wrap" style={`flex:${panelRatio}`}>
        <HexPanel
          filePath={pathA}
          fileSize={result.file_a.size}
          ranges={rangesA}
          topRow={topRow}
          onscrollto={onScroll}
          autofocus
          {activeOffset}
          onmodified={() => (dirty = true)}
          onedit={recordEditA}
          refresh={refreshA}
          onfirstline={finishLoading}
          initialChunk={initialA}
          onnavkey={handleNavKey}
          {dbgKey}
          onrowselect={onRowSelect}
          showHud={settings.showHud}
          side="A"
          searchHits={searchHitsA}
          searchActive={searchHitsA ? searchActiveA : -1}
          onselectrow={onSelectRow}
          syncSelection={rowSelection}
          scrollRows={Math.min(RENDER_CAP_ROWS, maxRows)}
          windowStart={winStart}
          windowSize={RENDER_CAP_ROWS}
          onwindowedge={shiftWindow}
          {pulse}
        />
      </div>
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div class="divider" role="separator" onmousedown={onDividerDown}></div>
      <div class="panel-wrap" style={`flex:${1 - panelRatio}`}>
        <HexPanel
          filePath={pathB}
          fileSize={result.file_b.size}
          ranges={rangesB}
          topRow={topRow}
          onscrollto={onScroll}
          {activeOffset}
          onmodified={() => (dirty = true)}
          onedit={recordEditB}
          refresh={refreshB}
          onfirstline={finishLoading}
          initialChunk={initialB}
          onnavkey={handleNavKey}
          {dbgKey}
          onrowselect={onRowSelect}
          showHud={settings.showHud}
          scrollRows={Math.min(RENDER_CAP_ROWS, maxRows)}
          windowStart={winStart}
          windowSize={RENDER_CAP_ROWS}
          onwindowedge={shiftWindow}
          {pulse}
          side="B"
          searchHits={searchHitsB}
          searchActive={searchHitsB ? searchActiveB : -1}
          onselectrow={onSelectRow}
          syncSelection={rowSelection}
        />
      </div>
    </div>

    <footer class="statusbar">
      <div class="nav">
        <button class="btn" onclick={prev} disabled={result.entries.length === 0}>← 上一处</button>
        <span class="pos">
          {result.entries.length === 0
            ? "无差异"
            : `${activeIndex + 1} / ${result.entries_truncated ? result.entries_total.toLocaleString() : result.entries.length}`}
          {activeChange ? ` · ${changeLabel[activeChange]}` : ""}
          {activeIndex >= 0 && result.entries[activeIndex]
            ? ` · ${offsetHex(result.entries[activeIndex])}`
            : ""}
        </span>
        <button class="btn" onclick={next} disabled={result.entries.length === 0}>下一处 →</button>
        <button
          class="btn"
          title="复制当前差异的十六进制偏移"
          onclick={() => {
            if (!result || activeIndex < 0) return;
            const e = result.entries[activeIndex];
            if (e) copyText(offsetHex(e));
          }}
          disabled={result.entries.length === 0}>⧉ 复制偏移</button>
      </div>
      <div class="sum">总计 {fmtSize(result.summary.total_bytes)}{elapsed > 0 ? ` · 耗时 ${elapsed.toFixed(1)}s` : ""}{#if result.entries_truncated}<span class="trunc"> · 仅显示前 {result.entries.length.toLocaleString()} / 共 {result.entries_total.toLocaleString()} 处差异（性能保护）</span>{/if}{#if result.strategy_fallback}<span class="fallback"> · sliding 策略发散，已自动回退 {result.strategy_used}（点击「重新比对」可强制使用原策略）</span>{/if}</div>
      {#if settings.showHud}
        <div class="dbg">KEY[{dbgKey}]</div>
      {/if}
    </footer>
  {:else}
    <div class="welcome">
      <div class="welcome-logo">
        <span class="welcome-mark">≠</span>
      </div>
      <h1 class="welcome-title">RVA 二进制差异比对</h1>
      <p class="welcome-sub">快速定位两个二进制文件之间的差异字节</p>
      <div class="welcome-actions">
        <button class="btn primary" onclick={browseBoth}>连续选择文件</button>
        <button class="btn" onclick={browseA}>打开文件 A</button>
        <button class="btn" onclick={browseB}>打开文件 B</button>
      </div>
      <div class="welcome-hint">
        <span>支持拖拽文件进窗口</span>
        <span>支持 PE / ELF / Mach-O</span>
        <span>拖动选择字节 Ctrl+C 复制</span>
        <span>双击十六进制进入编辑</span>
      </div>
    </div>
  {/if}
</div>

<style>
  .diff {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .welcome {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 40px;
    text-align: center;
    background:
      radial-gradient(900px 420px at 50% -10%, rgba(143, 124, 224, 0.14), transparent 65%),
      radial-gradient(700px 380px at 85% 110%, rgba(80, 220, 120, 0.06), transparent 60%),
      #0e1013;
    user-select: none;
  }
  .welcome-logo {
    width: 76px;
    height: 76px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 20px;
    background: linear-gradient(160deg, #1e2631, #151a21);
    border: 1px solid #2a313a;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4), inset 0 1px 0 rgba(255, 255, 255, 0.05);
  }
  .welcome-mark {
    font-size: 38px;
    font-weight: 800;
    color: #9b7dff;
    text-shadow: 0 0 24px rgba(155, 125, 255, 0.45);
  }
  .welcome-title {
    margin: 4px 0 0;
    font-size: 24px;
    font-weight: 700;
    letter-spacing: 0.4px;
    color: #eef1f5;
  }
  .welcome-sub {
    margin: 0;
    font-size: 13px;
    color: #7f8998;
  }
  .welcome-actions {
    display: flex;
    gap: 10px;
    margin-top: 10px;
    flex-wrap: wrap;
    justify-content: center;
  }
  .welcome-hint {
    display: flex;
    gap: 18px;
    margin-top: 22px;
    flex-wrap: wrap;
    justify-content: center;
    font-size: 12px;
    color: #5b6472;
  }
  .welcome-hint kbd {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 11px;
    padding: 1px 5px;
    border-radius: 4px;
    background: #1a1f27;
    border: 1px solid #2a313a;
    border-bottom-width: 2px;
    color: #aeb8c6;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    background: #12151a;
    border-bottom: 1px solid #232830;
  }
  .path-row {
    display: flex;
    flex: 1;
    gap: 6px;
    min-width: 0;
  }
  .path-input.a:focus { border-color: #ff5d5d; box-shadow: 0 0 0 3px rgba(255, 93, 93, 0.16); }
  .path-input.b:focus { border-color: #4ade80; box-shadow: 0 0 0 3px rgba(74, 222, 128, 0.16); }

  .dirty {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    background: rgba(242, 193, 78, 0.1);
    color: #f2c14e;
    font-size: 13px;
    border-bottom: 1px solid rgba(242, 193, 78, 0.18);
  }
  .dirty .btn { padding: 4px 10px; font-size: 12px; }

  .progress-track {
    position: relative;
    height: 3px;
    background: #1c222b;
    overflow: hidden;
    flex-shrink: 0;
  }
  .progress-bar {
    position: absolute;
    top: 0;
    left: 0;
    width: 0;
    height: 100%;
    background: linear-gradient(90deg, #6f5cc4, #9b7dff);
    box-shadow: 0 0 8px rgba(155, 125, 255, 0.55);
    transition: width 0.2s ease;
  }

  .fileinfo {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 16px;
    background: #111419;
    border-bottom: 1px solid #232830;
    font-size: 12px;
    color: #8b94a2;
  }
  .finfo {
    display: flex;
    gap: 12px;
    font-variant-numeric: tabular-nums;
  }
  .fname { color: #e8ebef; font-weight: 600; }
  .fname.a { color: #ff5d5d; }
  .fname.b { color: #4ade80; }

  .badges { display: flex; gap: 8px; }
  .badge-wrap {
    position: relative;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 999px;
    border: 1px solid transparent;
    font-variant-numeric: tabular-nums;
    cursor: pointer;
    user-select: none;
    transition: background 0.15s ease, border-color 0.15s ease, filter 0.15s ease;
  }
  .badge.added { color: #4ade80; background: rgba(74, 222, 128, 0.12); }
  .badge.removed { color: #ff5d5d; background: rgba(255, 93, 93, 0.12); }

  .badge:hover { filter: brightness(1.15); }
  .badge.added:hover { border-color: rgba(74, 222, 128, 0.35); }
  .badge.removed:hover { border-color: rgba(255, 93, 93, 0.35); }

  .badge-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex-shrink: 0;
    box-shadow: 0 0 6px currentColor;
  }
  .badge.added .badge-dot { background: #4ade80; }
  .badge.removed .badge-dot { background: #ff5d5d; }

  .badge-num { font-weight: 700; }
  .chev {
    color: rgba(255, 255, 255, 0.45);
    flex-shrink: 0;
    transition: transform 0.18s ease;
  }
  .badge-wrap:hover .chev { transform: rotate(180deg); }

  .diff-pop {
    position: absolute;
    top: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    width: 430px;
    max-width: 80vw;
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    background: #161c24;
    border: 1px solid #2c3542;
    border-radius: 12px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.65), 0 0 0 1px rgba(0, 0, 0, 0.3);
    z-index: 50;
    overflow: hidden;
    animation: pop-in 0.14s ease-out;
    transform-origin: top center;
  }
  @keyframes pop-in {
    from { opacity: 0; transform: translateX(-50%) scale(0.96) translateY(-4px); }
    to { opacity: 1; transform: translateX(-50%) scale(1) translateY(0); }
  }
  .pop-head {
    padding: 9px 14px;
    font-size: 12px;
    font-weight: 600;
    color: #b6bfcb;
    border-bottom: 1px solid #232b36;
    background: #131820;
    flex-shrink: 0;
    letter-spacing: 0.2px;
  }
  .pop-list {
    overflow-y: auto;
    padding: 6px;
  }
  .pop-empty {
    padding: 20px 12px;
    text-align: center;
    font-size: 12px;
    color: #6b7482;
  }
  /* -- Task F: entry container (whole row wraps the expanded details) -- */
  .pop-entry {
    border-radius: 8px;
    border-left: 3px solid transparent;
    transition: background 0.1s ease;
  }
  .pop-entry:hover { background: rgba(255, 255, 255, 0.04); }
  .pop-entry.expanded { background: rgba(255, 255, 255, 0.06); }
  .pop-entry.added { border-left-color: #4ade80; }
  .pop-entry.removed { border-left-color: #ff5d5d; }

  .pop-entry.active { background: rgba(143, 124, 224, 0.22); }

  .pop-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    border: none;
    background: transparent;
    color: #d5dbe3;
    font-size: 13px;
    text-align: left;
    border-radius: 8px;
    cursor: pointer;
    transition: background 0.12s ease;
  }
  .pop-item:hover { background: transparent; }

  /* Expand/collapse arrow: rotates 90deg when expanded */
  .pop-arrow {
    color: #8b8f9c;
    flex-shrink: 0;
    transition: transform 0.2s cubic-bezier(0.4, 0, 0.2, 1);
    transform-origin: center center;
  }
  .pop-entry.expanded .pop-arrow { transform: rotate(90deg); }

  .pop-idx {
    color: #6b7482;
    font-size: 12px;
    min-width: 28px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .pop-off {
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    color: #aeb8c6;
    font-size: 12px;
  }
  .pop-len {
    margin-left: auto;
    color: #6b7482;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }
  .pop-copy {
    color: #6b7482;
    font-size: 13px;
    padding: 2px 6px;
    border-radius: 4px;
    flex-shrink: 0;
  }
  .pop-copy:hover { color: #e8ebef; background: #2a313a; }

  /* -- Task F: expanded details (two-column byte comparison) -- */
  .pop-detail {
    margin: 0 8px 8px 8px;
    padding: 10px 12px;
    background: #10151c;
    border: 1px solid #232b36;
    border-radius: 8px;
    animation: detail-in 0.2s cubic-bezier(0.4, 0, 0.2, 1);
    transform-origin: top center;
  }
  @keyframes detail-in {
    from { opacity: 0; transform: translateY(-6px); }
    to { opacity: 1; transform: translateY(0); }
  }
  .pop-detail-loading {
    padding: 6px 0;
    font-size: 12px;
    color: #6b7482;
    text-align: center;
  }
  .byte-cols {
    display: flex;
    gap: 12px;
    align-items: stretch;
  }
  .byte-col {
    flex: 1;
    min-width: 0;
    padding: 8px 10px;
    border-radius: 6px;
  }
  .byte-col.old {
    background: rgba(255, 93, 93, 0.08);
    border-left: 2px solid #ff5d5d;
  }
  .byte-col.new {
    background: rgba(74, 222, 128, 0.08);
    border-left: 2px solid #4ade80;
  }
  .byte-col.none {
    background: #131820;
    border-left: 2px solid #2a313a;
  }
  .byte-col-title {
    font-size: 11px;
    font-weight: 600;
    color: #8b93a0;
    margin-bottom: 8px;
    letter-spacing: 0.2px;
  }
  .byte-col-hint {
    font-size: 11px;
    color: #6b7482;
    text-align: center;
    padding: 8px 0;
  }
  .byte-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    max-height: 160px;
    overflow-y: auto;
  }
  .byte-cell {
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    font-size: 12px;
    color: #e8eaf0;
    background: transparent;
    border-radius: 2px;
    transition: background 0.1s ease;
  }
  .byte-cell:hover { background: rgba(143, 124, 224, 0.15); }
  .byte-cell.same { color: #9aa3b0; }
  .byte-cell.diff {
    background: rgba(143, 124, 224, 0.16);
    color: #e8eaf0;
  }

  .toolrow {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 16px;
    background: #12151a;
    border-bottom: 1px solid #232830;
    flex-wrap: wrap;
  }
  .toolrow .btn { padding: 5px 10px; font-size: 12px; }
  .sep {
    width: 1px;
    height: 20px;
    background: #2a313a;
    margin: 0 4px;
    flex-shrink: 0;
  }
  .settings-wrap { position: relative; }
  .settings-wrap a.btn {
    text-decoration: none;
    display: inline-flex;
    align-items: center;
    vertical-align: middle;
  }
  .search-n {
    color: #6b7482;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    min-width: 44px;
    text-align: center;
  }
  .diff-nav {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .diff-nav .btn { padding: 5px 9px; }
  .diff-n {
    color: #9aa4b2;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    min-width: 48px;
    text-align: center;
  }

  .panels {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  .panel-wrap {
    display: flex;
    min-width: 0;
    overflow: hidden;
  }
  .divider {
    width: 5px;
    background: #1a1f26;
    flex-shrink: 0;
    cursor: col-resize;
    transition: background 0.15s;
  }
  .divider:hover { background: #2f6fed; }

  .statusbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 16px;
    background: #161a1f;
    border-top: 1px solid #232830;
    font-size: 12px;
  }
  .nav { display: flex; align-items: center; gap: 12px; }
  .pos {
    color: #9aa4b2;
    font-variant-numeric: tabular-nums;
    min-width: 120px;
    text-align: center;
  }
  .sum { color: #6b7482; font-variant-numeric: tabular-nums; }
  .trunc { color: #d99a3a; margin-left: 4px; }
  .dbg { color: #5db98f; font-size: 11px; font-family: monospace; max-width: 38%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
