<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { SymbolDto } from "./types";

  let { pathA, pathB }: { pathA: string; pathB: string } = $props();

  let side = $state<"A" | "B">("A");
  let symbols = $state<SymbolDto[]>([]);
  let loading = $state(false);
  let error = $state("");
  let filter = $state("");

  const currentPath = $derived(side === "A" ? pathA : pathB);
  const filtered = $derived(
    filter ? symbols.filter((s) => s.name.toLowerCase().includes(filter.toLowerCase())) : symbols,
  );

  async function load() {
    if (!currentPath) return;
    loading = true;
    error = "";
    try {
      symbols = await invoke<SymbolDto[]>("symbols", { path: currentPath });
    } catch (e) {
      error = String(e);
      symbols = [];
    } finally {
      loading = false;
    }
  }

  const fmtAddr = (n: number) => "0x" + n.toString(16).toUpperCase().padStart(8, "0");
  const fmtSize = (n: number | null) => (n === null ? "—" : n + " B");
</script>

<div class="view">
  <header class="viewbar">
    <div class="title">符号表</div>
    <div class="seg">
      <button class:on={side === "A"} onclick={() => (side = "A")}>A</button>
      <button class:on={side === "B"} onclick={() => (side = "B")}>B</button>
    </div>
    <input class="path-input grow" placeholder="过滤符号…" bind:value={filter} />
    <button class="btn primary" onclick={load} disabled={loading || !currentPath}>
      {loading ? "解析中…" : "解析符号"}
    </button>
  </header>

  <div class="pathbar">{currentPath || "请先在比对视图选择文件"}</div>

  {#if error}
    <div class="error">{error}</div>
  {/if}

  <div class="table-wrap">
    <table>
      <thead>
        <tr><th>地址</th><th>名称</th><th>大小</th></tr>
      </thead>
      <tbody>
        {#each filtered as s (s.addr)}
          <tr>
            <td class="mono">{fmtAddr(s.addr)}</td>
            <td class="name">{s.name}</td>
            <td class="mono dim">{fmtSize(s.size)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if symbols.length === 0 && !loading && !error}
      <div class="empty">无符号信息（裸二进制或未导出符号的文件为空）。</div>
    {/if}
  </div>
</div>

<style>
  .seg { display: flex; background: #0d0f12; border: 1px solid #232830; border-radius: 6px; overflow: hidden; }
  .seg button { background: none; border: none; color: #8b94a2; padding: 5px 12px; cursor: pointer; font-size: 13px; }
  .seg button.on { background: #333846; color: #9b7dff; }
  .pathbar { padding: 6px 12px; color: #6b7482; font-size: 12px; border-bottom: 1px solid #232830; font-family: Consolas, monospace; }
  .table-wrap { flex: 1; overflow: auto; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  thead th { position: sticky; top: 0; background: #171a1f; color: #8b94a2; text-align: left; padding: 6px 12px; border-bottom: 1px solid #232830; font-weight: 500; }
  tbody td { padding: 4px 12px; border-bottom: 1px solid #1b1f26; color: #d6dae0; }
  tbody tr:hover td { background: #1a1f26; }
  .mono { font-family: Consolas, monospace; }
  .name { font-family: Consolas, monospace; color: #a8d1ff; }
  .dim { color: #6b7482; }
</style>
