<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import type { BatchNodeDto } from "./types";
  import BatchNodeRow from "./BatchNodeRow.svelte";

  let dirA = $state("");
  let dirB = $state("");
  let root = $state<BatchNodeDto | null>(null);
  let loading = $state(false);
  let error = $state("");

  async function browseA() {
    const d = await open({ directory: true, title: "选择目录 A" });
    if (d && !Array.isArray(d)) dirA = d;
  }
  async function browseB() {
    const d = await open({ directory: true, title: "选择目录 B" });
    if (d && !Array.isArray(d)) dirB = d;
  }

  async function compare() {
    if (!dirA || !dirB) return;
    loading = true;
    error = "";
    try {
      root = await invoke<BatchNodeDto>("batch_compare", { dirA, dirB });
    } catch (e) {
      error = String(e);
      root = null;
    } finally {
      loading = false;
    }
  }
</script>

<div class="view">
  <header class="viewbar">
    <div class="title">批量比对</div>
    <input class="path-input grow" placeholder="目录 A" bind:value={dirA} />
    <button class="btn" onclick={browseA}>浏览</button>
    <input class="path-input grow" placeholder="目录 B" bind:value={dirB} />
    <button class="btn" onclick={browseB}>浏览</button>
    <button class="btn primary" onclick={compare} disabled={loading || !dirA || !dirB}>
      {loading ? "比对中…" : "比对"}
    </button>
  </header>

  {#if error}
    <div class="error">{error}</div>
  {/if}

  <div class="tree">
    {#if root}
      <BatchNodeRow node={root} depth={0} />
    {:else if !loading}
      <div class="empty">选择两个目录进行递归比对。</div>
    {/if}
  </div>
</div>

<style>
  .tree { flex: 1; overflow: auto; padding: 8px; }
</style>
