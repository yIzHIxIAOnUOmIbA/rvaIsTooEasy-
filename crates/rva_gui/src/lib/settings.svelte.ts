// Global settings store (Svelte 5 runes, module-level $state).
// All settings apply instantly and persist to localStorage under the key rva_settings_<snake_case> (spec convention, e.g. rva_settings_bytes_per_row).
// Backward compat: showHud keeps the legacy key rva_show_hud ("1" = on).
// The main view (DiffView / HexPanel) and the settings page share this store; changes trigger a reactive re-render.
//
// Usage:
//   import { settings, setSetting } from "$lib/settings.svelte";
//   read: settings.bytesPerRow        (reactive)
//   write: setSetting("bytesPerRow", 20)

export type DiffStrategy = "sliding" | "structural" | "chunked";
export type AddrBase = "hex" | "dec";
export type HexCase = "lower" | "upper";
/** Copy format: matches the top toolbar dropdown (shared by DiffView toolbar and HexPanel Ctrl+C/right-click, unified copy path P0-2) */
export type CopyFormat = "hex" | "hexsp" | "carr" | "rarr" | "py" | "ascii";
export type SettingKey = keyof typeof DEFAULTS;

export const DEFAULTS = {
  // — Display panel —
  bytesPerRow: 16, // bytes per row, 8–32, step 4
  hexFontSize: 14, // hex font size, 12–18px
  showAscii: true, // show ASCII side panel
  showGutter: true, // show line-number gutter
  addrBase: "hex" as AddrBase, // address base: hex/dec
  hexCase: "lower" as HexCase, // hex case: lower/upper
  copyFormat: "hexsp" as CopyFormat, // copy format: hexsp/hex/carr/rarr/py/ascii
  // — Diff panel —
  diffStrategy: "chunked" as DiffStrategy, // default strategy (matches main view default)
  slidingWindow: 16, // sliding window size, 4–64
  autoJumpFirst: true, // auto-jump to first diff after compare
  badgeHoverDelay: 300, // diff badge hover delay, ms, 0–1000
  // — Debug panel —
  showHud: false, // HUD debug toggle (legacy key rva_show_hud)
  engineLog: false, // output engine logs to the console
} as const;

/** Map camelCase setting -> localStorage key (rva_settings_ + snake_case) */
const KEY_OF: Record<SettingKey, string> = {
  bytesPerRow: "rva_settings_bytes_per_row",
  hexFontSize: "rva_settings_hex_font_size",
  showAscii: "rva_settings_show_ascii",
  showGutter: "rva_settings_show_gutter",
  addrBase: "rva_settings_addr_base",
  hexCase: "rva_settings_hex_case",
  copyFormat: "rva_settings_copy_format",
  diffStrategy: "rva_settings_diff_strategy",
  slidingWindow: "rva_settings_sliding_window",
  autoJumpFirst: "rva_settings_auto_jump_first",
  badgeHoverDelay: "rva_settings_badge_hover_delay",
  showHud: "rva_show_hud", // legacy key, bound to existing logic
  engineLog: "rva_settings_engine_log",
};

/** Group settings by panel (used by the "Restore defaults" button to reset per panel) */
export const GROUPS: { id: string; label: string; keys: SettingKey[] }[] = [
  {
    id: "display",
    label: "Display",
    keys: ["bytesPerRow", "hexFontSize", "showAscii", "showGutter", "addrBase", "hexCase", "copyFormat"],
  },
  {
    id: "diff",
    label: "Diff",
    keys: ["diffStrategy", "slidingWindow", "autoJumpFirst", "badgeHoverDelay"],
  },
  { id: "debug", label: "Debug", keys: ["showHud", "engineLog"] },
];

/** Parse boolean/string stored values */
function parseStored<T>(raw: string | null, fallback: T, cast: (v: string) => T): T {
  if (raw === null || raw === undefined || raw === "") return fallback;
  try {
    return cast(raw);
  } catch {
    return fallback;
  }
}

function merge(): typeof DEFAULTS {
  const out = { ...DEFAULTS } as typeof DEFAULTS;
  const g = (k: SettingKey) => KEY_OF[k];
  out.bytesPerRow = parseStored(localStorage.getItem(g("bytesPerRow")), DEFAULTS.bytesPerRow, (v) => {
    const n = Number(v);
    return n >= 8 && n <= 32 && n % 4 === 0 ? n : DEFAULTS.bytesPerRow;
  });
  out.hexFontSize = parseStored(localStorage.getItem(g("hexFontSize")), DEFAULTS.hexFontSize, (v) => {
    const n = Number(v);
    return n >= 12 && n <= 18 ? n : DEFAULTS.hexFontSize;
  });
  out.showAscii = parseStored(localStorage.getItem(g("showAscii")), DEFAULTS.showAscii, (v) => v === "1");
  out.showGutter = parseStored(localStorage.getItem(g("showGutter")), DEFAULTS.showGutter, (v) => v === "1");
  out.addrBase = parseStored(localStorage.getItem(g("addrBase")), DEFAULTS.addrBase, (v) =>
    v === "dec" ? "dec" : "hex",
  );
  out.hexCase = parseStored(localStorage.getItem(g("hexCase")), DEFAULTS.hexCase, (v) =>
    v === "upper" ? "upper" : "lower",
  );
  out.copyFormat = parseStored(localStorage.getItem(g("copyFormat")), DEFAULTS.copyFormat, (v) =>
    v === "hex" || v === "hexsp" || v === "carr" || v === "rarr" || v === "py" || v === "ascii"
      ? (v as CopyFormat)
      : DEFAULTS.copyFormat,
  );
  out.diffStrategy = parseStored(localStorage.getItem(g("diffStrategy")), DEFAULTS.diffStrategy, (v) =>
    v === "sliding" || v === "structural" || v === "chunked" ? v : DEFAULTS.diffStrategy,
  );
  out.slidingWindow = parseStored(localStorage.getItem(g("slidingWindow")), DEFAULTS.slidingWindow, (v) => {
    const n = Number(v);
    return n >= 4 && n <= 64 ? n : DEFAULTS.slidingWindow;
  });
  out.autoJumpFirst = parseStored(localStorage.getItem(g("autoJumpFirst")), DEFAULTS.autoJumpFirst, (v) => v === "1");
  out.badgeHoverDelay = parseStored(localStorage.getItem(g("badgeHoverDelay")), DEFAULTS.badgeHoverDelay, (v) => {
    const n = Number(v);
    return n >= 0 && n <= 1000 ? n : DEFAULTS.badgeHoverDelay;
  });
  out.showHud = parseStored(localStorage.getItem(g("showHud")), DEFAULTS.showHud, (v) => v === "1");
  out.engineLog = parseStored(localStorage.getItem(g("engineLog")), DEFAULTS.engineLog, (v) => v === "1");
  return out;
}

/** Unified copy serialization: Hex spaced/compact, C/Rust arrays, Python bytes, ASCII (non-printable -> ".").
 *  DiffView toolbar "Copy" and HexPanel Ctrl+C / right-click share the same format, so both produce identical output (P0-2). */
export function formatBytes(data: number[], format: CopyFormat): string {
  const hx = (b: number) => b.toString(16).padStart(2, "0").toUpperCase();
  switch (format) {
    case "hex":
      return data.map(hx).join("");
    case "hexsp":
      return data.map(hx).join(" ");
    case "carr":
      return "{ " + data.map((b) => `0x${hx(b)}`).join(", ") + " }";
    case "rarr":
      return "[" + data.map((b) => `0x${hx(b)}`).join(", ") + "]";
    case "py":
      return `b"${data.map((b) => `\\x${hx(b)}`).join("")}"`;
    case "ascii":
      return data.map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
  }
}

/** Global reactive settings object: reading a field subscribes; writes must go through setSetting */
export const settings: typeof DEFAULTS = $state(merge());

function persistKey(key: SettingKey) {
  try {
    const v = (settings as Record<string, unknown>)[key];
    if (typeof v === "boolean") {
      localStorage.setItem(KEY_OF[key], v ? "1" : "0");
    } else {
      localStorage.setItem(KEY_OF[key], String(v));
    }
  } catch {
    /* ignore */
  }
}

/** Write a single setting: update reactive state and persist (no save button, applies instantly) */
export function setSetting<K extends SettingKey>(key: K, value: (typeof DEFAULTS)[K]) {
  (settings as Record<string, unknown>)[key] = value;
  persistKey(key);
}

/** Restore defaults per panel group: reset all keys in the group to their defaults and persist */
export function resetGroup(groupId: string) {
  const g = GROUPS.find((x) => x.id === groupId);
  if (!g) return;
  for (const k of g.keys) {
    (settings as Record<string, unknown>)[k] = DEFAULTS[k];
    persistKey(k);
  }
}
