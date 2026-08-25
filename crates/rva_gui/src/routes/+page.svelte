<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import DiffView from "$lib/DiffView.svelte";
  import SymbolsView from "$lib/SymbolsView.svelte";
  import PatchView from "$lib/PatchView.svelte";
  import BatchView from "$lib/BatchView.svelte";
  import ReportView from "$lib/ReportView.svelte";
  import SettingsView from "$lib/SettingsView.svelte";

  type ViewId = "diff" | "symbols" | "patch" | "batch" | "report" | "settings";

  // Task A: initial-route redirect — when "Don't show again" is unchecked, the first entry to the main view redirects to the welcome page.
  // A sessionStorage flag prevents a re-redirect on "Welcome -> Start -> back to main view".
  onMount(() => {
    try {
      if (sessionStorage.getItem("rva_welcome_passed") !== "1") {
        sessionStorage.setItem("rva_welcome_passed", "1");
        if (localStorage.getItem("rva_skip_welcome") !== "1") {
          goto("/welcome");
        }
      }
    } catch {
      /* ignore */
    }
  });

  let activeView = $state<ViewId>("diff");
  let pathA = $state("");
  let pathB = $state("");

  const views: { id: ViewId; icon: string; label: string }[] = [
    { id: "diff", icon: "≠", label: "比对" },
    { id: "symbols", icon: "ƒ", label: "符号" },
    { id: "patch", icon: "⊕", label: "补丁" },
    { id: "batch", icon: "▦", label: "批量" },
    { id: "report", icon: "≡", label: "报告" },
    { id: "settings", icon: "⚙", label: "设置" },
  ];
</script>

<div class="shell">
  <aside class="sidebar">
    <div class="logo">RVA<span>.</span></div>
    <nav class="nav">
      {#each views as v (v.id)}
        <button
          class="nav-item"
          class:active={activeView === v.id}
          onclick={() => (activeView = v.id)}
          title={v.label}
        >
          <span class="ico">{v.icon}</span>
          <span class="lbl">{v.label}</span>
        </button>
      {/each}
    </nav>
    <div class="side-foot">RVA Compare</div>
  </aside>

  <main class="main">
    {#if activeView === "diff"}
      <DiffView bind:pathA bind:pathB onopensettings={() => (activeView = "settings")} />
    {:else if activeView === "symbols"}
      <SymbolsView {pathA} {pathB} />
    {:else if activeView === "patch"}
      <PatchView bind:pathA bind:pathB />
    {:else if activeView === "batch"}
      <BatchView />
    {:else if activeView === "settings"}
      <SettingsView />
    {:else}
      <ReportView {pathA} {pathB} />
    {/if}
  </main>
</div>

<style>
  :global(body) {
    margin: 0;
    font-family: "Segoe UI", system-ui, -apple-system, sans-serif;
    background: #1a1d23;
    color: #d8dbe2;
  }

  /* — Global shared styles (across all views) — */
  :global(.view) {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  :global(.viewbar) {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    background: #12151a;
    border-bottom: 1px solid #232830;
    flex-wrap: wrap;
  }
  :global(.title) {
    font-size: 14px;
    font-weight: 600;
    color: #e8ebef;
    white-space: nowrap;
  }
  :global(.grow) { flex: 1; min-width: 0; }
  :global(.mono) { font-family: Consolas, monospace; }
  :global(.ellip) { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  :global(.btn) {
    background: #252a32;
    border: 1px solid #3c414c;
    color: #d8dbe2;
    border-radius: 6px;
    padding: 7px 14px;
    font-size: 13px;
    cursor: pointer;
    white-space: nowrap;
    transition:
      background 0.15s ease,
      border-color 0.15s ease,
      color 0.15s ease,
      transform 0.08s ease,
      box-shadow 0.15s ease;
  }
  :global(.btn:hover:not(:disabled)) { background: #2e333d; border-color: #4a4f5b; }
  :global(.btn:active:not(:disabled)) { transform: translateY(2px); }
  :global(.btn:focus-visible) {
    outline: none;
    box-shadow: 0 0 0 3px rgba(155, 125, 255, 0.35);
  }
  :global(.btn:disabled) { opacity: 0.45; cursor: not-allowed; }
  /* Primary button: purple family but toned down (lower saturation/brightness so it doesn't steal visual focus).
     The purple highlight on selected rows/items lives in HexPanel's own styles and is unaffected by this. */
  :global(.btn.primary) {
    background: linear-gradient(180deg, #6f639c, #453a66);
    border-color: #6f5f9e;
    color: #e9e4f5;
    font-weight: 700;
    text-shadow: 0 1px 2px rgba(20, 10, 40, 0.35);
    box-shadow: 0 2px 8px rgba(70, 55, 110, 0.3), inset 0 1px 0 rgba(255, 255, 255, 0.14);
  }
  :global(.btn.primary:hover:not(:disabled)) {
    background: linear-gradient(180deg, #7c6fb0, #524580);
    border-color: #8a78b8;
    box-shadow: 0 4px 16px rgba(120, 100, 190, 0.35), inset 0 1px 0 rgba(255, 255, 255, 0.18);
    transform: translateY(-1px);
  }
  :global(.btn.primary:active:not(:disabled)) {
    background: linear-gradient(180deg, #38304f, #2a2340);
    border-color: #524878;
    color: #cfc8e2;
    text-shadow: none;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.4), inset 0 3px 8px rgba(10, 5, 25, 0.65);
    transform: translateY(3px) scale(0.99);
  }
  /* Keep the purple tone while comparing/disabled, so full-opacity graying doesn't make it feel unclickable. */
  :global(.btn.primary:disabled) {
    opacity: 0.75;
    background: linear-gradient(180deg, #5f5690, #403866);
    border-color: #554b80;
    color: #c6bfdd;
    box-shadow: none;
  }
  /* Keyboard Tab focus: explicit highlight ring on the primary button (avoids being overridden by .btn.primary's box-shadow). */
  :global(.btn.primary:focus-visible) {
    box-shadow: 0 0 0 3px rgba(155, 125, 255, 0.55);
  }

  :global(.path-input) {
    flex: 1;
    background: #14161b;
    border: 1px solid #3c414c;
    border-radius: 6px;
    color: #d8dbe2;
    padding: 7px 10px;
    font-size: 13px;
    font-family: Consolas, monospace;
    outline: none;
    transition: border-color 0.15s ease, box-shadow 0.15s ease;
  }
  :global(.path-input:focus) { border-color: #9b7dff; box-shadow: 0 0 0 3px rgba(155, 125, 255, 0.22); }

  :global(.mini) {
    width: 130px;
    background: #14161b;
    border: 1px solid #3c414c;
    border-radius: 6px;
    color: #d8dbe2;
    padding: 5px 8px;
    font-size: 12px;
    font-family: Consolas, monospace;
    outline: none;
    transition: border-color 0.15s ease, box-shadow 0.15s ease;
  }
  :global(.mini:focus) { border-color: #9b7dff; box-shadow: 0 0 0 3px rgba(155, 125, 255, 0.22); }
  :global(.mini.search) { width: 190px; }

  :global(.sel) {
    background: #14161b;
    border: 1px solid #3c414c;
    border-radius: 6px;
    color: #d8dbe2;
    padding: 5px 6px;
    font-size: 12px;
    outline: none;
    transition: border-color 0.15s ease, box-shadow 0.15s ease;
  }
  :global(.sel:focus) { border-color: #9b7dff; box-shadow: 0 0 0 3px rgba(155, 125, 255, 0.22); }

  :global(.error) {
    padding: 8px 16px;
    background: rgba(255, 86, 86, 0.12);
    color: #ff8585;
    font-size: 13px;
    font-family: Consolas, monospace;
    border-bottom: 1px solid rgba(255, 86, 86, 0.2);
  }

  :global(.empty) {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    color: #8b94a2;
    gap: 6px;
    padding: 24px;
    text-align: center;
  }
  :global(.empty p) { margin: 0; font-size: 15px; }
  :global(.empty .hint) { font-size: 12px; color: #5b6472; }

  /* — Shell layout — */
  .shell { display: flex; height: 100vh; overflow: hidden; }
  .sidebar {
    width: 132px;
    background: #20242b;
    border-right: 1px solid #2e333c;
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
  }
  .logo {
    padding: 18px 0 14px;
    text-align: center;
    font-size: 20px;
    font-weight: 700;
    letter-spacing: 0.5px;
    color: #e8ebef;
  }
  .logo span { color: #9b7dff; }
  .nav {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 8px;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 12px;
    border: none;
    border-radius: 6px;
    background: none;
    color: #8b94a2;
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }
  .nav-item:hover { background: #2a2f38; color: #d3d8e0; }
  .nav-item.active { background: #333846; color: #f0edf8; }
  .nav-item.active .ico { color: #9b7dff; }
  /* Keyboard Tab focus: the sidebar nav gets a clear highlight feedback. */
  .nav-item:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px rgba(155, 125, 255, 0.55);
    background: #2a2f38;
  }
  .ico { font-size: 16px; width: 20px; text-align: center; }
  .lbl { font-weight: 500; }
  .side-foot {
    padding: 10px;
    text-align: center;
    font-size: 11px;
    color: #5c6472;
    border-top: 1px solid #2e333c;
  }
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    background: #1a1d23;
  }
</style>
