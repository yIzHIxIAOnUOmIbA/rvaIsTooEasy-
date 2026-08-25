<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, untrack, tick } from "svelte";
  import type { HighlightRange } from "./types";
  import { settings, formatBytes } from "./settings.svelte";

  interface Props {
    filePath: string;
    fileSize: number;
    ranges: HighlightRange[];
    topRow?: number;
    // Notifies the parent when the user scrolls this panel (the parent syncs the other panel to keep relative offsets)
    onscrollto?: (row: number) => void;
    // Whether to auto-focus after mount/file switch (only the left panel A enables this, avoiding both panels fighting for focus and breaking the keyboard)
    autofocus?: boolean;
    activeOffset: number;
    onmodified?: () => void;
    onedit?: (info: { offset: number; oldByte: number; newByte: number }) => void;
    refresh?: number;
    onfirstline?: () => void;
    // Preloaded by the parent in the compare-button (user gesture) context and passed in directly, bypassing the issue of invoke responses getting stuck inside $effect
    initialChunk?: Uint8Array | null;
    // Keyboard navigation fallback: while the container holds focus, the parent handles arrow keys/paging (the parent also
    // listens globally in the document/window capture phase; this is a second layer of insurance)
    onnavkey?: (e: KeyboardEvent) => void;
    // The parent's latest key-press diagnostics, shown directly in the bottom-left HUD to observe whether key events arrive
    dbgKey?: string;
    // Reports the first-byte offset of a clicked row; the parent syncs activeIndex, the status-bar number, and the hit-row highlight
    onrowselect?: (offset: number) => void;
    // Whether to show the bottom-left HUD debug bar (chunk loading / scroll diagnostics), controlled by a parent setting
    showHud?: boolean;
    // -- Task E: double-click the line number to select the row --
    // This panel's id (A/B), used to differentiate highlight opacity between the primary side and the passive sync side
    side?: "A" | "B";
    // Notifies the parent when double-clicking the line-number gutter selects a whole row / clears a selection (the parent syncs the other panel)
    onselectrow?: (sel: { start: number; end: number } | null) => void;
    // The whole-row selection range synced from the parent (the opposite panel renders a dimmed highlight from it)
    syncSelection?: { start: number; end: number } | null;
    // Global scroll rows (max of the two files): both panels share the same topRow, so even when this file is shorter the
    // scrollbar must still span the large file's height, otherwise the small-file panel bottoms out first and the rest of the large file becomes unreachable
    scrollRows?: number;
    // Performance guard: scroll window (absolute first row + window row count, 0=unlimited). Outside the window nothing is
    // rendered or scrolled; scrolling to the window edge calls onwindowedge to switch windows; out-of-range content is entered via the parent's jump feature
    windowStart?: number;
    windowSize?: number;
    onwindowedge?: (dir: "prev" | "next") => void;
    // Jump-target pulse hint: the parent updates this on entry/search/row jumps (with an incremented key),
    // and this panel plays a double-flash animation (same color as the selected row) to clearly show where the jump landed
    pulse?: { row: number; key: number } | null;
    // Incremented by the parent on jumps/recomputes: this panel clears its local selection and context menu,
    // preventing the opposite panel's stale row highlight from lingering after "jump to next"
    clearSelectionKey?: number;
    // Search hits (search-side panel only): {offset,len} list + current active index (-1 for none).
    // Used for byte-exact highlight of matches (replacing whole-row pulse flashing, avoiding "neighboring bytes also matched by mistake")
    searchHits?: { offset: number; len: number }[] | null;
    searchActive?: number;
  }

  let {
    filePath,
    fileSize,
    ranges,
    topRow = 0,
    onscrollto = () => {},
    autofocus = false,
    activeOffset,
    onmodified,
    onedit,
    refresh = 0,
    onfirstline,
    initialChunk = null,
    onnavkey,
    dbgKey = "",
    onrowselect,
    showHud = true,
    side = "A",
    onselectrow,
    syncSelection = null,
    scrollRows = 0,
    windowStart = 0,
    windowSize = 0,
    onwindowedge,
    pulse = null,
    clearSelectionKey = 0,
    searchHits = null,
    searchActive = -1,
  }: Props = $props();

  // -- Task R: jump-target pulse hint --
  // When the parent updates pulse (with an incremented key), play a double-flash animation whose colors match the selected-row highlight.
  let pulseRow = $state(-1);
  let pulseAlpha = $state(0);
  let pulseRaf = 0;
  $effect(() => {
    const p = pulse;
    if (!p) return;
    pulseRow = p.row;
    const t0 = performance.now();
    if (pulseRaf) cancelAnimationFrame(pulseRaf);
    const tick = (now: number) => {
      const t = now - t0;
      const dur = 700; // total duration of the two flashes
      if (t >= dur) {
        pulseAlpha = 0;
        pulseRaf = 0;
        draw();
        return;
      }
      const ph = t % 350; // each flash is 350ms: 190ms on + 160ms off
      pulseAlpha = ph < 190 ? ph / 190 : Math.max(0, 1 - (ph - 190) / 160);
      draw();
      pulseRaf = requestAnimationFrame(tick);
    };
    pulseRaf = requestAnimationFrame(tick);
  });

  // Task 4: bytes per row is now reactive (controlled by the settings page, default 16)
  const BYTES_PER_ROW = $derived(settings.bytesPerRow);
  const ROW_HEIGHT = 20;
  const CHUNK_SIZE = 65536; // 64KB per chunk: fewer chunks and IPC calls, less cache fragmentation
  const CHUNK_CACHE_LIMIT = 256; // frontend chunk-cache cap; evict FIFO beyond it to prevent unbounded memory growth

  let container: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let editInput: HTMLInputElement | undefined = undefined;
  // Custom double-click detection: preventDefault on mousedown suppresses the browser's dblclick, so we cannot rely on the ondblclick event
  let lastClickTime = 0;
  let lastClickOffset = -1;

  let viewportHeight = $state(0);
  let viewportWidth = $state(0);
  // Key fix: must be $state, otherwise Svelte 5 won't trigger redraws/re-evaluation of the dependency chain after chunks.set,
  // causing "chunk loaded but the view still shows .." and the old implementation's pending never being cleaned up (a deadlock).
  let chunks = $state(new Map<number, Uint8Array>());
  let chunkVersion = $state(0);
  // Incremented on file switch/reload to discard stale in-flight chunk request results
  let chunkGen = 0;
  // This panel's actually displayed row (derived from its own scrollTop and clamped per panel), avoiding blank/misaligned output when the shared topRow goes out of range
  let displayRow = $state(0);
  // Chunk-loading diagnostics state
  let pendingChunks = $state(new Set<number>());
  let lastChunkError = $state<string | null>(null);
  // Diagnostics: whether the first-screen preload arrived (initialChunk from DiffView)
  let initialArrived = $state<"pending" | "ok" | "empty">("pending");
  // Synced scrolling: when the program pulls this panel's scrollTop toward the shared topRow, record the "program target".
  // In onScroll, if scrollTop lands exactly on that target, it is a sync echo and topRow is not written back;
  // otherwise it is treated as the user scrolling, and topRow is written back. This way, even while the other panel keeps
  // scrolling and topRow is already at a newer value, this panel's lagging echo cannot overwrite it with a stale value -- desync is fully eliminated.
  let programTarget = -1;
  // Diagnostics: record scroll/sync key values in real time, shown on the green bar to locate "desync / key not working"
  let dbgScroll = $state(""); // latest onScroll: st / echo / whether emit
  let dbgEffect = $state(""); // latest effect: k / whether scrollTop was set
  let prevPath: string | null = null;
  // Whether the first on-screen line is ready (to notify the parent to finish the progress bar); reset on file switch/reload
  let firstLineFired = false;

  const MONO_FONT = "13px 'Cascadia Mono', Consolas, 'Courier New', monospace";

  let editing = $state<{
    offset: number;
    x: number;
    y: number;
    value: string;
  } | null>(null);

  // Plain-text-style selection: selects the byte range [start, end] (inclusive)
  let selection = $state<{ start: number; end: number } | null>(null);
  let dragging = $state(false);
  let dragAnchor = 0;
  // Whether the drag-selection started in the ASCII area: when copying, if so and all bytes are printable, copy plain text instead of hex
  let dragAnchorInAscii = false;
  // Context menu (standalone control panel): pops up when a byte is right-clicked, with copy/edit actions; right-clicking also clears the selection
  let ctxMenu = $state<{ x: number; y: number; offset: number } | null>(null);

  // Even when fileSize is missing/invalid (undefined, 0), still produce a valid total row count, so NaN does not propagate up
  // through firstRow/lastRow and trigger the chain failure "first-line signal never fires -> progress bar frozen forever, chunks never load"
  const totalRows = $derived(Math.max(1, Math.ceil((fileSize || 0) / BYTES_PER_ROW)));
  // Scroll range = max of this file's rows and the global rows (fixes the small-file panel blocking large-file scrolling).
  // Use totalRows rather than the in-window row count: the scrollbar covers the whole file, dragging stays continuous without snapping back;
  // the window only decides the render range, window switching is driven by the parent's ensureWindow based on scroll position, and scrollTop is never reset.
  const scrollRowsEff = $derived(Math.max(totalRows, scrollRows || 0));
  // Large-file scroll-range compression: the browser caps scrollTop at ~33.5M px; beyond that the scrollbar breaks and later content cannot be shown.
  // spacer height = scrollRowsEff×effRowHeight (full content height); scroll range = content height - viewport height,
  // so when scrolled to the bottom the file's last row hugs the viewport's bottom edge, leaving no full-screen blank after the file end;
  // shared coordinates (topRow/onscrollto) still use real pixels, and this panel's internal scrollTop = real pixels × scrollScale.
  const MAX_SCROLL_PX = 30_000_000;
  const scrollScale = $derived(
    scrollRowsEff * ROW_HEIGHT > MAX_SCROLL_PX ? MAX_SCROLL_PX / (scrollRowsEff * ROW_HEIGHT) : 1
  );
  const effRowHeight = $derived(ROW_HEIGHT * scrollScale);
  const firstRow = $derived(Math.min(displayRow, totalRows - 1));
  const visibleRows = $derived(Math.ceil(viewportHeight / ROW_HEIGHT) + 2);
  const lastRow = $derived(Math.min(firstRow + visibleRows, totalRows));

  // Aggregated diagnostics string, shown directly on the green bar to avoid depending on the DevTools console.
  // Prefixed with the build-time injected unique version (vite define); it differs per build, used to tell new builds from old.
  declare const __APP_VERSION__: string;
  let diag = $derived(
    `v${__APP_VERSION__} | fp=${filePath ? filePath.slice(-12) : "-"} size=${fileSize} rows=${totalRows} ` +
      `chunks=${chunks.size} pending=${pendingChunks.size} h=${viewportHeight} ` +
      `init=${initialArrived}:${initialChunk ? initialChunk.length : "null"} ` +
      `err=${lastChunkError ? lastChunkError.slice(0, 40) : "-"} ` +
      `| SCROLL[${dbgScroll}] FX[${dbgEffect}] KEY[${dbgKey}]`,
  );

  function measure() {
    if (!container) return;
    viewportHeight = container.clientHeight;
    viewportWidth = container.clientWidth;
    // Redraw once when the viewport size is ready, so the first screen does not wait for a later event (scrolling) to show content
    draw();
    // Fallback: if the first visible chunk is empty (initialChunk missed), proactively load one to avoid a blank view without any refresh
    ensureVisibleChunks();
  }

  /** Requests all not-yet-loaded chunks in the current visible range. Shared by measure/onscroll. */
  function ensureVisibleChunks() {
    if (fileSize === undefined || fileSize === null) return;
    const fc = Math.floor((firstRow * BYTES_PER_ROW) / CHUNK_SIZE);
    const lc = Math.floor(((lastRow - 1) * BYTES_PER_ROW) / CHUNK_SIZE);
    for (let c = fc; c <= lc; c++) {
      if (!chunks.has(c)) loadChunk(c);
    }
  }

  // Keyboard paging is handled uniformly by DiffView in the document capture phase (focus-independent; see DiffView.onKeyNav).
  // This panel only keeps the onMount focus so that focus lands on the panel after opening a file, easing other interactions.

  onMount(() => {
    measure();
    // The first layout may not be stable yet; measure again on the next frame to avoid a 0-sized canvas causing a blank first screen
    requestAnimationFrame(measure);
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    // After mounting, proactively give focus to the panel's scroll container (left panel A only): after opening a file the arrow keys work immediately without clicking the panel first.
    if (autofocus) {
      requestAnimationFrame(() => {
        if (container && document.activeElement !== container) {
          container.focus({ preventScroll: true });
        }
      });
    }
    return () => {
      ro.disconnect();
    };
  });

  // File switch: clear the cache and go back to the top. On first mount there is no stale state to clean, so return early --
  // avoiding chunkGen++ marking the first-screen loadChunk requests already issued in onMount/measure as stale and dropped.
  $effect(() => {
    const fp = filePath;
    if (prevPath === fp) return;
    const isFirst = prevPath === null;
    prevPath = fp;
    if (isFirst) return;
    chunks = new Map();
    chunkGen++;
    firstLineFired = false;
    displayRow = 0;
    if (container) container.scrollTop = 0;
    // After the new file is ready, hand focus back to the container so the keyboard works immediately (left panel A only)
    if (autofocus) {
      requestAnimationFrame(() => {
        if (container && document.activeElement !== container) {
          container.focus({ preventScroll: true });
        }
      });
    }
  });

  // After an external undo writes the file back, reload the visible area
  // Depend only on refresh; all internal writes (chunks/chunkVersion, etc.) are wrapped in untrack,
  // avoiding "write chunkVersion -> triggers the effect depending on it -> writes again -> infinite synchronous re-entry".
  $effect(() => {
    if (refresh === 0) return;
    untrack(() => {
      chunks = new Map();
      chunkGen++;
      firstLineFired = false;
      chunkVersion++;
      const fc = Math.floor((firstRow * BYTES_PER_ROW) / CHUNK_SIZE);
      const lc = Math.floor(((lastRow - 1) * BYTES_PER_ROW) / CHUNK_SIZE);
      for (let c = fc; c <= lc; c++) loadChunk(c);
    });
  });

  // The first-screen chunk preloaded by the parent (user-gesture context): write the data into the matching chunk only when initialChunk changes.
  // Key fix: read topRow/chunks inside untrack -- otherwise this effect treats the chunks it wrote as dependencies again,
  // forming an infinite "read chunks -> write chunks -> trigger again" loop; meanwhile every scroll (topRow change) would re-run
  // and mis-write the first-screen chunk into the currently visible chunk, overwriting the correct data loaded by loadChunk and corrupting the bottom view.
  $effect(() => {
    const fp = filePath; // establish the dependency: after a file switch the effect re-evaluates if initialChunk is still stale data
    const ic = initialChunk;
    if (ic && ic.length > 0) {
      // Depend only on initialChunk/filePath; all writes (chunks/chunkVersion, etc.) are untracked,
      // otherwise the chunkVersion written here would trigger the effect depending on it, which triggers itself again -- a synchronous re-entry loop.
      untrack(() => {
        const c = Math.floor(Math.floor(topRow / ROW_HEIGHT) * BYTES_PER_ROW / CHUNK_SIZE);
        const next = new Map(chunks);
        next.set(c, ic);
        chunks = next;
        initialArrived = "ok";
        chunkVersion++;
        draw();
      });
    } else if (ic) {
      initialArrived = "empty"; // the preload returned an empty array (file may be empty or the read failed)
    }
  });

  async function loadChunk(c: number) {
    if (pendingChunks.has(c)) return;
    const gen = chunkGen;
    pendingChunks = new Set([...pendingChunks, c]);
    try {
      const data = await invoke<number[]>("read_bytes", {
        path: filePath,
        offset: c * CHUNK_SIZE,
        length: CHUNK_SIZE,
      });
      if (gen !== chunkGen) return; // the file was switched meanwhile; the stale result is discarded
      const next = new Map(chunks);
      next.set(c, new Uint8Array(data));
      // FIFO eviction: drop the earliest loaded chunk when over the cap, preventing unbounded memory growth while scrolling a large file back and forth
      if (next.size > CHUNK_CACHE_LIMIT) {
        const oldest = next.keys().next().value;
        if (oldest !== undefined) next.delete(oldest);
      }
      chunks = next;
      // When the first on-screen line (the chunk containing the first visible byte) is ready, notify the parent to complete the progress bar
      if (!firstLineFired) {
        const visByte = firstRow * BYTES_PER_ROW;
        const start = c * CHUNK_SIZE;
        if (visByte >= start && visByte < start + CHUNK_SIZE) {
          firstLineFired = true;
          onfirstline?.();
        }
      }
      chunkVersion++;
      // Redraw immediately after data arrives, avoiding "chunk arrived but the view did not refresh" caused by effect scheduling delays/failures
      draw();
    } catch (e) {
      // A read failure must not stay silent: it would leave the first-line signal never firing and the UI frozen
      lastChunkError = String(e);
      console.error("[HexPanel] read_bytes failed:", filePath, c, e);
    } finally {
      pendingChunks = new Set([...pendingChunks].filter((x) => x !== c));
    }
  }

  // Shared row number (topRow) changed -> sync this panel's pixel position. Before setting scrollTop, record the program
  // target so onScroll can detect the echo (see the programTarget note above).
  $effect(() => {
    if (container) {
      // Compressed-domain scrollTop cap and target (the shared topRow is in real pixels; multiply by scrollScale to get the compressed domain).
      // spacer = scrollRowsEff×effRowHeight (full content height); scroll range cap = content height - viewport height,
      // so at the bottom the last row hugs the viewport's bottom edge, leaving no full-screen blank after the file end.
      const maxScrollST = Math.max(0, scrollRowsEff * effRowHeight - viewportHeight);
      const clamped = Math.min(Math.max(0, topRow * scrollScale), maxScrollST);
      if (Math.abs(container.scrollTop - clamped) > 1) {
        dbgEffect = `k=${topRow} was=${container.scrollTop}->${clamped} max=${maxScrollST}`;
        programTarget = clamped;
        container.scrollTop = clamped;
      } else {
        dbgEffect = `k=${topRow} cur=${container.scrollTop} skip`;
      }
    }
    displayRow = Math.min(Math.floor(Math.max(0, topRow) / ROW_HEIGHT), Math.max(0, totalRows - 1));
  });

  // Scrolling: distinguish "the user scrolled this panel" from "a sync effect pulled it (echo)".
  // Echo detection: if this panel's scrollTop lands exactly on programTarget (the target value just set by the sync effect),
  // it was pulled by the program and topRow is not written back; otherwise the user scrolled this panel and topRow is set to this panel's position.
  function onScroll() {
    if (!container) return;
    ctxMenu = null; // close the context menu on scroll to avoid a floating, mis-positioned menu
    const st = container.scrollTop;
    // compressed-domain scrollTop -> real pixels (when uncompressed scrollScale=1 and st is already real pixels)
    const abs = st / scrollScale;
    displayRow = Math.min(Math.floor(st / effRowHeight), Math.max(0, totalRows - 1));
    const isEcho = programTarget >= 0 && Math.abs(st - programTarget) <= 2;
    programTarget = -1; // either way, this target value has been consumed
    if (isEcho) {
      dbgScroll = `st=${st} echo=1 emit=0 k=${topRow}`;
      if (editing) editing = null;
      ensureVisibleChunks();
      draw();
      return;
    }
    const emit = Math.abs(abs - topRow) > 1;
    if (emit) {
      onscrollto(abs);
    }
    dbgScroll = `st=${st} echo=${isEcho ? 1 : 0} emit=${emit ? 1 : 0} k=${topRow}`;
    if (editing) editing = null;
    // Whether user scroll or echo, load the visible chunks at this panel's current position and redraw
    ensureVisibleChunks();
    draw();
  }

  function getLayout() {
    const ctx = canvas?.getContext("2d");
    const fallback = 7.8;
    if (!ctx) {
      return {
        charW: fallback,
        offsetX: 8,
        hexX: settings.showGutter ? 8 + 9 * fallback : 8,
        asciiX: 8 + 58 * fallback,
      };
    }
    ctx.font = MONO_FONT;
    const charW = ctx.measureText("0").width;
    const offsetX = 8;
    // Task 4: when the gutter is off, the hex area starts right from the left margin (no reserved address-column width)
    const hexX = settings.showGutter ? offsetX + 9 * charW : offsetX;
    const asciiX = hexX + 49 * charW;
    return { charW, offsetX, hexX, asciiX };
  }

  function hitTestAt(clientX: number, clientY: number): { offset: number; inAscii: boolean } | null {
    if (!filePath || !container) return null;
    const rect = container.getBoundingClientRect();
    const vx = clientX - rect.left;
    const py = clientY - rect.top;
    const { charW, hexX, asciiX } = getLayout();
    // Strictly the inverse of draw's row mapping. In draw, when uncompressed row y = row*ROW_HEIGHT - st (st=floor(scrollTop)),
    // so row = floor((py+st)/ROW_HEIGHT); when compressed row y = (row-fr)*ROW_HEIGHT, so row = fr+floor(py/ROW_HEIGHT).
    // A sub-pixel scroll remainder (st%20≠0) would make "fr+floor(py/20)" hit one row too high overall, so the inverse formula is required.
    const st = Math.floor(container.scrollTop);
    const fr = Math.floor(st / effRowHeight);
    const row = scrollScale < 1
      ? fr + Math.floor(py / ROW_HEIGHT)
      : Math.floor((py + st) / ROW_HEIGHT);
    let col = -1;
    let inAscii = false;
    if (vx >= hexX && vx < asciiX - charW) {
      col = Math.floor((vx - hexX) / (3 * charW));
    } else if (settings.showAscii && vx >= asciiX) {
      col = Math.floor((vx - asciiX) / charW);
      inAscii = true;
    }
    if (col < 0 || col >= BYTES_PER_ROW) return null;
    const offset = row * BYTES_PER_ROW + col;
    if (offset >= fileSize) return null;
    return { offset, inAscii };
  }

  // gutter y coordinate -> absolute row (strictly consistent with draw's row mapping: uncompressed uses the inverse formula to avoid the sub-pixel scroll remainder shifting row hits overall)
  function rowAtY(clientY: number): number {
    if (!container) return -1;
    const rect = container.getBoundingClientRect();
    const st = Math.floor(container.scrollTop);
    const fr = Math.floor(st / effRowHeight);
    return scrollScale < 1
      ? fr + Math.floor((clientY - rect.top) / ROW_HEIGHT)
      : Math.floor((clientY - rect.top + st) / ROW_HEIGHT);
  }

  // Like plain text: press/drag to select, double-click to edit
  function onMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    ctxMenu = null; // left-click anywhere closes the context menu
    if (editing) return;
    // Give the container focus so container-level keydown works in the Tauri WebView; does not affect hitTest/selection/editing
    if (container && document.activeElement !== container) {
      container.focus({ preventScroll: true });
    }
    const rect = container.getBoundingClientRect();
    const { hexX } = getLayout();
    // Line-number gutter: click selects the whole row; dragging selects contiguous rows (row granularity)
    const inGutter = settings.showGutter && e.clientX - rect.left < hexX;
    let anchorRow = -1;
    if (inGutter) {
      const row = rowAtY(e.clientY);
      if (row < 0 || row >= totalRows) return;
      const start = row * BYTES_PER_ROW;
      const end = Math.min(start + BYTES_PER_ROW - 1, fileSize - 1);
      if (start > end) return;
      e.preventDefault();
      anchorRow = row;
      selection = { start, end };
      dragging = true;
      dragAnchor = start;
      dragAnchorInAscii = false;
    } else {
      const hit = hitTestAt(e.clientX, e.clientY);
      if (hit === null) {
        // Clicking blank space (outside the hex/ascii areas): keep the current selection, avoiding "after double-clicking a row,
        // clicking elsewhere loses progress". Clearing the selection is left to clicking the byte area (starting a new selection).
        return;
      }
      // Custom double-click detection: preventDefault on mousedown suppresses the browser's dblclick event,
      // so ondblclick cannot be relied on (otherwise a double-click counts as two clicks -> clears the selection and editing fails).
      const now = Date.now();
      const isDouble = hit.offset === lastClickOffset && now - lastClickTime < 400;
      lastClickTime = now;
      lastClickOffset = hit.offset;
      if (isDouble) {
        e.preventDefault();
        if (selection || syncSelection) {
          selection = null;
          onselectrow?.(null);
        }
        startEdit(hit.offset);
        return;
      }
      // Both clicking a byte and drag-selecting start a new selection: clear any previous whole-row selection (including the
      // opposite-side sync), avoiding two selected rows after "double-clicking a row then clicking another diff row".
      if (selection || syncSelection) {
        selection = null;
        onselectrow?.(null);
      }
      e.preventDefault();
      dragging = true;
      dragAnchor = hit.offset;
      dragAnchorInAscii = hit.inAscii;
      selection = { start: hit.offset, end: hit.offset };
    }
    // Shared drag-selection move/release: gutter row granularity <-> byte-area byte granularity, reported to the parent in real
    // time during the drag so both A/B sides render identically (no longer two independently computed selections)
    let lastReported = "";
    const move = (ev: MouseEvent) => {
      if (!dragging) return;
      const r = container.getBoundingClientRect();
      const vx = ev.clientX - r.left;
      if (settings.showGutter && vx < hexX) {
        const row = rowAtY(ev.clientY);
        if (row < 0 || row >= totalRows) return;
        const aRow = anchorRow >= 0 ? anchorRow : Math.floor(dragAnchor / BYTES_PER_ROW);
        const s = Math.min(aRow, row) * BYTES_PER_ROW;
        const en = Math.min(Math.max(aRow, row) * BYTES_PER_ROW + BYTES_PER_ROW - 1, fileSize - 1);
        selection = { start: s, end: en };
      } else {
        const h = hitTestAt(ev.clientX, ev.clientY);
        if (h === null) return;
        selection = {
          start: Math.min(dragAnchor, h.offset),
          end: Math.max(dragAnchor, h.offset),
        };
      }
      // Sync the opposite side in real time during the drag (deduplicated), keeping A/B drag-selected rows linked consistently
      const key = `${selection.start}-${selection.end}`;
      if (key !== lastReported) {
        lastReported = key;
        onselectrow?.({ start: selection.start, end: selection.end });
      }
    };
    const up = () => {
      dragging = false;
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      // Dragging into a selection range (cross-byte/cross-row) = starting a whole-range selection: report it to the parent so the
      // opposite panel highlights in sync and the copy button lights up; the opposite side renders a dimmed highlight via
      // syncSelection, guaranteeing only one selection exists globally at a time
      if (selection && selection.start !== selection.end) {
        onselectrow?.({ start: selection.start, end: selection.end });
        // A gutter click (row not dragged) also syncs the status bar
        if (anchorRow >= 0) onrowselect?.(selection.start);
      } else if (selection) {
        // A click (no drag into a range) = select the row: report the row's first-byte offset, and the parent syncs the status-bar number and highlight
        onrowselect?.(selection.start);
      }
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  }

  // clearSelectionKey is incremented by the parent on jumps/recomputes: this panel clears its local selection and context
  // menu, preventing the opposite panel's stale row highlight from lingering after "jump to next".
  $effect(() => {
    const k = clearSelectionKey ?? 0;
    if (k > 0) {
      selection = null;
      ctxMenu = null;
    }
  });

  function startEdit(offset: number) {
    const b = getByte(offset);
    if (b === null) return;
    const { charW, hexX } = getLayout();
    const row = Math.floor(offset / BYTES_PER_ROW);
    const col = offset % BYTES_PER_ROW;
    const st = Math.floor(container.scrollTop);
    const fr = Math.floor(st / effRowHeight);
    const compressed = scrollScale < 1;
    const y = compressed ? (row - fr) * ROW_HEIGHT : row * ROW_HEIGHT - st;
    editing = {
      offset,
      x: hexX + col * 3 * charW,
      y,
      value: b.toString(16).padStart(2, "0"),
    };
    // Focus fallback: on the first double-click the second mousedown already locked focus on the container, so focus must be
    // taken back only after the input mounts, otherwise under Tauri/Chromium the input never gets focus -> "the box appears but cannot be edited".
    tick().then(() => {
      if (editInput) {
        editInput.focus();
        editInput.select();
      }
    });
  }

  async function copySelection() {
    const sel = selection;
    if (!sel) return;
    const bytes: number[] = [];
    for (let off = sel.start; off <= sel.end; off++) {
      let b = getByte(off);
      if (b === null) {
        await loadChunk(Math.floor(off / CHUNK_SIZE));
        b = getByte(off);
      }
      if (b !== null) bytes.push(b);
    }
    if (bytes.length === 0) return;
    // If the drag selection started in the ASCII area: copy as WYSIWYG plain text (exactly matching the ASCII area display),
    // with non-printable bytes mapped to '.'
    if (dragAnchorInAscii) {
      const text = bytes
        .map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "."))
        .join("");
      await copyText(text);
      return;
    }
    // All others (hex-area drag selection / Ctrl+C / context-menu copy) follow the global copy format settings.copyFormat,
    // matching the top toolbar's "Copy" dropdown -- P0-2 unified copy pipeline, the two copy paths always produce identical output
    await copyText(formatBytes(bytes, settings.copyFormat));
  }

  // Right-click: disable the browser default menu; a hit byte pops the standalone action panel.
  // Does not alter the byte selection state (right-click only summons the action panel, never mixes with left-click
  // selection/drag, and avoids the chain issue "after right-click the opposite sync is cleared, and a later left-click on a byte re-pops the panel").
  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const hit = hitTestAt(e.clientX, e.clientY);
    if (!hit || !container) return;
    const rect = container.getBoundingClientRect();
    // The menu is about 168×132; clamp it inside the viewport so it is not clipped outside the panel
    ctxMenu = {
      x: Math.max(0, Math.min(e.clientX - rect.left, viewportWidth - 168)),
      y: Math.max(0, Math.min(e.clientY - rect.top, viewportHeight - 132)),
      offset: hit.offset,
    };
  }

  function closeCtxMenu() {
    ctxMenu = null;
  }

  // Context-menu copy: copies the whole row containing the hit byte.
  // "format" = per the global copy format (settings.copyFormat, matching the top toolbar); "ascii" = plain text
  function ctxCopy(kind: "format" | "ascii") {
    const off = ctxMenu?.offset;
    if (off === undefined) return;
    const start = Math.floor(off / BYTES_PER_ROW) * BYTES_PER_ROW;
    const end = Math.min(start + BYTES_PER_ROW - 1, fileSize - 1);
    dragAnchorInAscii = kind === "ascii";
    const sel = { start, end };
    selection = sel;
    ctxMenu = null;
    copySelection().finally(() => {
      // Clear only if no new drag/selection happened during the copy, so finally does not wipe a selection the user just started
      if (selection === sel) selection = null;
    });
  }

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      /* Silently fail when the clipboard is unavailable */
    }
  }

  async function commitEdit() {
    const ed = editing;
    if (!ed) return;
    const v = ed.value.trim();
    if (/^[0-9a-fA-F]{1,2}$/.test(v)) {
      const byte = parseInt(v, 16);
      const old = getByte(ed.offset);
      try {
        await invoke("write_bytes", {
          path: filePath,
          offset: ed.offset,
          data: [byte],
        });
        const c = Math.floor(ed.offset / CHUNK_SIZE);
        const bytes = chunks.get(c);
        if (bytes) {
          bytes[ed.offset - c * CHUNK_SIZE] = byte;
          chunkVersion++;
        }
        // Content actually changed (old known and different) -> record undo and trigger the parent to re-compare automatically;
        // "delete then fill back" (old === byte) does not trigger, avoiding a false need to re-compare;
        // chunk missing (old === null, rare) -> trigger the auto-recompute fallback (the diff engine reads from disk)
        if (old !== null && old !== byte) {
          onedit?.({ offset: ed.offset, oldByte: old, newByte: byte });
        } else if (old === null) {
          onmodified?.();
        }
      } catch {
        /* Write-to-disk failure: cancel silently */
      }
    }
    editing = null;
  }

  function cancelEdit() {
    editing = null;
  }

  function onEditKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      commitEdit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelEdit();
    }
  }

  function focusEditor(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  // Task 4: hex bytes follow the settings' case (copying still always uses uppercase)
  function hexByte(b: number): string {
    const s = b.toString(16).padStart(2, "0");
    return settings.hexCase === "upper" ? s.toUpperCase() : s;
  }

  function getByte(offset: number): number | null {
    const c = Math.floor(offset / CHUNK_SIZE);
    const bytes = chunks.get(c);
    if (!bytes) return null;
    const idx = offset - c * CHUNK_SIZE;
    return idx < bytes.length ? bytes[idx] : null;
  }

  /** Binary search for the range overlapping [byteOffset, byteOffset+len). */
  function rangeFor(byteOffset: number, len: number): HighlightRange | null {
    let lo = 0;
    let hi = ranges.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (ranges[mid].end <= byteOffset) lo = mid + 1;
      else hi = mid;
    }
    if (lo < ranges.length && ranges[lo].start < byteOffset + len) {
      return ranges[lo];
    }
    return null;
  }

  // Redraw (depends on visible rows, data chunks, highlight ranges, selection).
  // Note: viewportWidth/Height are intentionally NOT dependencies -- measure() writes them and Svelte treats that as a
  // synchronous update, so reading them here would form a "measure->redraw->measure" re-entry loop;
  // viewport size changes are already covered by measure() calling draw() directly.
  $effect(() => {
    displayRow;
    chunkVersion;
    ranges;
    activeOffset;
    selection;
    syncSelection;
    searchHits;
    searchActive;
    // Task 4: settings changes (bytes per row / font / ASCII / gutter / address radix / case) trigger an immediate redraw
    settings.bytesPerRow;
    settings.hexFontSize;
    settings.showAscii;
    settings.showGutter;
    settings.addrBase;
    settings.hexCase;
    draw();
  });

  // Task E: when the passive sync side receives a whole-row selection, auto-scroll to the selected row (viewport center, smooth 300ms).
  // Pre-clear: when the opposite side starts a new synced selection (key changed), clear this side's leftover local selection,
  // avoiding the double highlight "A selects row1 -> B selects row2 -> panel A lights row1 (local) + row2 (synced)".
  // Only judged on syncSelection changes (selection is not listened to), so it does not disturb this side's own drag-select.
  let lastSyncKey = "";
  $effect(() => {
    const sel = syncSelection;
    const key = sel ? `${sel.start}-${sel.end}` : "";
    if (sel && lastSyncKey && key !== lastSyncKey) {
      selection = null;
    }
    lastSyncKey = key;
  });

  $effect(() => {
    const sel = syncSelection;
    if (!sel || !container) return;
    const row = Math.floor(sel.start / BYTES_PER_ROW);
    const target = row * ROW_HEIGHT - (viewportHeight - ROW_HEIGHT) / 2;
    // Real-pixel domain cap (matching the spacer's full content height): at the bottom the last row hugs the viewport's bottom edge
    const maxScroll = Math.max(0, scrollRowsEff * ROW_HEIGHT - viewportHeight);
    const clamped = Math.max(0, Math.min(maxScroll, target));
    // Only scroll when the target row is outside the current viewport, avoiding pointless jumps
    const firstVis = Math.floor(container.scrollTop / effRowHeight);
    const lastVis = Math.ceil((container.scrollTop + viewportHeight) / effRowHeight);
    if (row < firstVis || row > lastVis) {
      container.scrollTo({ top: clamped * scrollScale, behavior: "smooth" });
    }
  });

  // Search-hit detection: returns 0=no hit 1=normal hit 2=currently-active hit
  function searchHitAt(off: number): number {
    if (!searchHits || searchHits.length === 0) return 0;
    const act = searchActive;
    for (let i = 0; i < searchHits.length; i++) {
      const h = searchHits[i];
      if (off >= h.offset && off < h.offset + h.len) return i === act ? 2 : 1;
    }
    return 0;
  }

  function draw() {
    const ctx = canvas?.getContext("2d");
    if (!ctx || !container || viewportWidth <= 0 || viewportHeight <= 0) return;

    const dpr = window.devicePixelRatio || 1;
    const w = viewportWidth;
    const h = viewportHeight;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    canvas.style.width = w + "px";
    canvas.style.height = h + "px";
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    ctx.fillStyle = "#14161a";
    ctx.fillRect(0, 0, w, h);

    ctx.font = MONO_FONT;
    const { charW, offsetX, hexX, asciiX } = getLayout();
    ctx.textBaseline = "top";

    // Follow the container's actual scroll position (including sub-pixels), avoiding the view jumping a row from whole-row snapping;
    // floor keeps fr strictly equal to displayRow(=floor(scrollTop/20)) and text crisp
    const st = Math.floor(container.scrollTop);
    const fr = Math.floor(st / effRowHeight);
    const vr = Math.ceil(viewportHeight / ROW_HEIGHT) + 2;
    const lr = Math.min(fr + vr, totalRows);
    const compressed = scrollScale < 1; // compressed mode snaps by row, avoiding drift from mixing compressed and real pixels
    for (let row = fr; row < lr; row++) {
      const y = compressed ? (row - fr) * ROW_HEIGHT : row * ROW_HEIGHT - st;
      const byteOffset = row * BYTES_PER_ROW;
      const r = rangeFor(byteOffset, BYTES_PER_ROW);

      if (r) {
        // Task R: diff rows get stronger background contrast -- deleted red / added green / modified yellow, easier to tell apart
        const color =
          r.kind === "Removed"
            ? "rgba(255,86,86,0.24)"
            : r.kind === "Added"
              ? "rgba(74,222,128,0.24)"
              : "rgba(0,0,0,0)";
        ctx.fillStyle = color;
        ctx.fillRect(0, y, w, ROW_HEIGHT);
      }

      const isActive =
        activeOffset >= byteOffset && activeOffset < byteOffset + BYTES_PER_ROW;
      if (isActive) {
        // Current diff-entry indicator: diff rows get the same color family intensified (after the pulse they restore to
        // green/red directly, leaving no purple "selected" residue); non-diff rows (a normal row was clicked) keep the faint
        // purple hint to distinguish "locate" from "select"
        if (r) {
          ctx.fillStyle =
            r.kind === "Removed"
              ? "rgba(255,86,86,0.32)"
              : r.kind === "Added"
                ? "rgba(74,222,128,0.32)"
                : "rgba(242,193,78,0.22)";
          ctx.fillRect(0, y, w, ROW_HEIGHT);
          ctx.fillStyle =
            r.kind === "Removed"
              ? "#ff5d5d"
              : r.kind === "Added"
                ? "#4ade80"
                : "#f2c14e";
          ctx.fillRect(0, y, 3, ROW_HEIGHT);
        } else {
          ctx.fillStyle = "rgba(155,125,255,0.12)";
          ctx.fillRect(0, y, w, ROW_HEIGHT);
          ctx.fillStyle = "#9b7dff";
          ctx.fillRect(0, y, 3, ROW_HEIGHT);
        }
      }

      // Jump-target pulse: two flashes (colors match the selected row); the target row is highlighted during the animation
      if (pulseRow === row && pulseAlpha > 0) {
        ctx.fillStyle = `rgba(155,125,255,${(0.28 * pulseAlpha).toFixed(3)})`;
        ctx.fillRect(0, y, w, ROW_HEIGHT);
        ctx.fillStyle = "#9b7dff";
        ctx.fillRect(0, y, 3, ROW_HEIGHT);
      }

      // -- Task E: whole-row selection (double-click the line number) three-layer visual feedback --
      // Primary side: the whole-row selection this panel started itself (the full row including both ends)
      // Passive sync side: the whole row synced in from the parent's syncSelection (lower opacity to tell them apart)
      const rowStart = byteOffset;
      const rowEnd = byteOffset + BYTES_PER_ROW - 1;
      const isRowPrimary =
        selection !== null &&
        selection.start <= rowStart &&
        selection.end >= rowEnd;
      const isRowSync =
        !isRowPrimary &&
        syncSelection !== null &&
        syncSelection.start <= rowStart &&
        syncSelection.end >= rowEnd;
      const rowSel = isRowPrimary ? selection : isRowSync ? syncSelection : null;
      if (rowSel) {
        // Layer 2: whole-row background band (covers the gutter and ASCII areas, forming a complete row-highlight band)
        // Both A/B sides use the same purple highlight (referencing the PVZ-Online Lobby selected state)
        ctx.fillStyle = "rgba(155,125,255,0.09)";
        ctx.fillRect(0, y, w, ROW_HEIGHT);
      }

      // Layer 3: the line-number gutter cell highlight (consistent bright-purple solid on both A/B sides + dark text)
      if (rowSel) {
        ctx.fillStyle = "#9b7dff";
        ctx.fillRect(0, y, hexX, ROW_HEIGHT);
        ctx.fillStyle = "#1a0f38";
      } else {
        ctx.fillStyle = "#5c6472";
      }
      // Task 4: gutter toggle + address radix (hex 8-digit / dec decimal)
      if (settings.showGutter) {
        const offTxt =
          settings.addrBase === "dec"
            ? byteOffset.toString(10)
            : byteOffset.toString(16).padStart(8, "0");
        ctx.fillText(offTxt, offsetX, y + 3);
      }

      for (let i = 0; i < BYTES_PER_ROW; i++) {
        const off = byteOffset + i;
        const b = getByte(off);
        const hx = hexX + i * 3 * charW;
        const ax = asciiX + i * charW;
        if (b === null) {
          // Chunk not loaded: draw a placeholder rather than a blank row; chunkVersion++ after loading triggers a redraw
          ctx.fillStyle = "#3a4048";
          ctx.fillText("..", hx, y + 3);
          if (settings.showAscii) ctx.fillText(".", ax, y + 3);
          continue;
        }
        // Search hits: byte-exact highlight of the matched bytes (normal hits pale yellow, the active hit bright yellow),
        // covering only len consecutive byte slots; bytes before/after the match never highlight
        const sh = searchHitAt(off);
        if (sh > 0) {
          ctx.fillStyle = sh === 2 ? "rgba(255,214,90,0.5)" : "rgba(255,214,90,0.26)";
          ctx.fillRect(hx - charW / 2, y, charW * 3, ROW_HEIGHT);
          if (settings.showAscii) ctx.fillRect(ax - charW / 2, y, charW * 1.5, ROW_HEIGHT);
        }
        if (rowSel && off >= rowSel.start && off <= rowSel.end) {
          // Layer 1: selected byte-cell background (drawn over the diff colors, semi-transparent so the layering stays visible)
          // Both A/B sides use the same purple highlight (referencing the PVZ-Online Lobby selected state #9B7DFF).
          // Box width: hex byte slots are spaced 3*charW apart, so the underline box takes 3*charW to connect seamlessly
          // end-to-end with the next byte's box (2.5*charW leaves a 0.5-char gap between bytes, visually "not filled"); ASCII takes 1.5*charW per char.
          ctx.fillStyle = "rgba(155,125,255,0.28)";
          ctx.fillRect(hx - charW / 2, y, charW * 3, ROW_HEIGHT);
          if (settings.showAscii) ctx.fillRect(ax - charW / 2, y, charW * 1.5, ROW_HEIGHT);
        } else if (selection && off >= selection.start && off <= selection.end) {
          ctx.fillStyle = "rgba(155,125,255,0.30)";
          ctx.fillRect(hx - charW / 2, y, charW * 3, ROW_HEIGHT);
          if (settings.showAscii) ctx.fillRect(ax - charW / 2, y, charW * 1.5, ROW_HEIGHT);
        }
        ctx.fillStyle = r ? "#f0f2f5" : "#c9cfd8";
        ctx.fillText(hexByte(b), hx, y + 3);
        if (settings.showAscii) {
          const ch = b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".";
          ctx.fillStyle = r ? "#b7bdc7" : "#6b7482";
          ctx.fillText(ch, ax, y + 3);
        }
      }
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<div class="hex-panel">
  <div
    class="hex-scroll"
    role="application"
    tabindex="0"
    bind:this={container}
    onscroll={onScroll}
    onmousedown={onMouseDown}
    oncontextmenu={onContextMenu}
    onkeydown={(e) => {
      // Selection copy: Ctrl/Cmd+C copies as hex text (space-separated)
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c" && selection) {
        copySelection();
        return;
      }
      // Task E: Esc clears the whole-row selection (including the opposite-side sync)
      if (e.key === "Escape" && (selection || syncSelection)) {
        selection = null;
        onselectrow?.(null);
        return;
      }
      onnavkey?.(e);
    }}
  >
    <div class="spacer" style="height: {scrollRowsEff * effRowHeight}px"></div>
  </div>
  <canvas bind:this={canvas}></canvas>
  {#if ctxMenu}
    <div
      class="ctx-menu"
      style="left: {ctxMenu.x}px; top: {ctxMenu.y}px;"
      onmousedown={(e) => e.stopPropagation()}
      oncontextmenu={(e) => e.preventDefault()}
    >
      <button onclick={() => ctxCopy("format")}>复制本行（按格式）</button>
      <button onclick={() => ctxCopy("ascii")}>复制 ASCII（纯文本）</button>
      <button
        onclick={() => {
          startEdit(ctxMenu.offset);
          ctxMenu = null;
        }}>编辑字节</button>
      <button class="dim" onclick={closeCtxMenu}>关闭</button>
    </div>
  {/if}
  {#if editing}
    <input
      class="hex-editor"
      bind:this={editInput}
      style="left: {editing.x}px; top: {editing.y}px;"
      bind:value={editing.value}
      onkeydown={onEditKeydown}
      onblur={commitEdit}
      onclick={(e) => e.stopPropagation()}
      onmousedown={(e) => e.stopPropagation()}
      maxlength={2}
      use:focusEditor
    />
  {/if}
  {#if showHud}
    <div class="chunk-debug">
      {diag}
    </div>
  {/if}
</div>

<style>
  /* The outer layer is only a positioning anchor and never scrolls; the canvas/HUD are its direct children, absolutely
     pinned to the visible area and never moving with scrolling. Scrolling is handled by the inner .hex-scroll, where the
     spacer provides the scroll height. */
  .hex-panel {
    position: relative;
    flex: 1;
    min-height: 0;
    background: #14161a;
  }
  .hex-scroll {
    position: absolute;
    inset: 0;
    overflow-y: auto;
    overflow-x: hidden;
    outline: none;
  }
  .hex-scroll:focus {
    outline: none;
  }
  .hex-scroll .spacer {
    position: relative;
    width: 1px;
  }
  .hex-panel canvas {
    position: absolute;
    top: 0;
    left: 0;
    pointer-events: none;
  }
  .chunk-debug {
    position: absolute;
    bottom: 4px;
    left: 4px;
    padding: 3px 8px;
    background: rgba(10, 13, 17, 0.82);
    color: #4ade80;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 11px;
    border: 1px solid rgba(74, 222, 128, 0.25);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
    letter-spacing: 0.2px;
    z-index: 10;
    pointer-events: none;
  }

  .hex-editor {
    position: absolute;
    width: 34px;
    height: 20px;
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 13px;
    background: #1a1330;
    color: #9b7dff;
    border: 1px solid #6b4daa;
    border-radius: 3px;
    padding: 0 2px;
    z-index: 10;
    outline: none;
    text-transform: uppercase;
  }

  .ctx-menu {
    position: absolute;
    z-index: 20;
    min-width: 156px;
    padding: 4px;
    background: #1b1e24;
    border: 1px solid #333a44;
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .ctx-menu button {
    text-align: left;
    padding: 6px 10px;
    background: transparent;
    border: none;
    border-radius: 5px;
    color: #d7dce3;
    font-size: 12px;
    cursor: pointer;
  }
  .ctx-menu button:hover {
    background: #2a2f38;
    color: #ffffff;
  }
  .ctx-menu button.dim {
    color: #8b93a1;
  }
</style>
