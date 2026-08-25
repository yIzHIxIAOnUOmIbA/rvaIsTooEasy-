<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { save } from "@tauri-apps/plugin-dialog";

  let { pathA, pathB }: { pathA: string; pathB: string } = $props();

  let format = $state<"html" | "txt" | "json">("html");
  let content = $state("");
  let loading = $state(false);
  let error = $state("");

  async function generate() {
    if (!pathA || !pathB) return;
    loading = true;
    error = "";
    try {
      content = await invoke<string>("export_report", { pathA, pathB, format });
    } catch (e) {
      error = String(e);
      content = "";
    } finally {
      loading = false;
    }
  }

  async function saveReport() {
    if (!content) return;
    const dst = await save({ title: "保存报告", defaultPath: `rva_report.${format}` });
    if (!dst) return;
    try {
      await invoke("write_text_file", { path: dst, content });
    } catch (e) {
      error = String(e);
    }
  }
</script>

<div class="view">
  <header class="viewbar">
    <div class="title">报告导出</div>
    <select class="sel" bind:value={format}>
      <option value="html">HTML</option>
      <option value="txt">TXT</option>
      <option value="json">JSON</option>
    </select>
    <button class="btn primary" onclick={generate} disabled={loading || !pathA || !pathB}>
      {loading ? "生成中…" : "生成报告"}
    </button>
    <button class="btn" onclick={saveReport} disabled={!content}>保存文件</button>
  </header>

  {#if error}
    <div class="error">{error}</div>
  {/if}

  <div class="preview">
    {#if content}
      {#if format === "html"}
        <iframe title="report" src={`data:text/html;charset=utf-8,${encodeURIComponent(content)}`}></iframe>
      {:else}
        <pre>{content}</pre>
      {/if}
    {:else}
      <div class="empty">请先在比对视图选好两个文件，再生成报告。</div>
    {/if}
  </div>
</div>

<style>
  .preview { flex: 1; overflow: auto; display: flex; flex-direction: column; }
  iframe { flex: 1; border: none; background: #fff; }
  pre { margin: 0; padding: 16px; color: #d6dae0; font-family: Consolas, monospace; font-size: 13px; white-space: pre-wrap; }
</style>
