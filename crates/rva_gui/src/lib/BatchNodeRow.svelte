<script lang="ts">
  import type { BatchNodeDto } from "./types";
  import BatchNodeRow from "./BatchNodeRow.svelte";

  let { node, depth }: { node: BatchNodeDto; depth: number } = $props();
  let expanded = $state(true);

  const label = $derived((node.path_a || node.path_b || "").split(/[\\/]/).pop() || "/");

  function toggle() {
    expanded = !expanded;
  }

  function statusText(s: string): string {
    switch (s) {
      case "Identical": return "相同";
      case "Different": return "差异";
      case "OnlyInA": return "仅A";
      case "OnlyInB": return "仅B";
      case "Error": return "错误";
      default: return s;
    }
  }
</script>

<div class="node" style={`padding-left: ${depth * 16}px`}>
  <div
    class="row"
    role="button"
    tabindex="0"
    onclick={toggle}
    onkeydown={(e) => e.key === "Enter" && toggle()}
  >
    <span class="arrow">{node.children.length ? (expanded ? "▾" : "▸") : "·"}</span>
    <span class="name">{label}</span>
    {#if node.status === "Different" && node.diff_count !== null}
      <span class="count">{node.diff_count} diffs</span>
    {/if}
    <span
      class="status"
      class:identical={node.status === "Identical"}
      class:different={node.status === "Different"}
      class:onlya={node.status === "OnlyInA"}
      class:onlyb={node.status === "OnlyInB"}
      class:error={node.status === "Error"}
    >{statusText(node.status)}</span>
  </div>
  {#if expanded}
    {#each node.children as child}
      <BatchNodeRow node={child} depth={depth + 1} />
    {/each}
  {/if}
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 8px;
    cursor: pointer;
    border-radius: 4px;
    font-size: 13px;
  }
  .row:hover { background: #1a1f26; }
  .arrow { width: 14px; color: #6b7482; flex-shrink: 0; }
  .name { color: #d6dae0; font-family: Consolas, monospace; }
  .count { color: #f0c850; font-size: 12px; }
  .status { margin-left: auto; font-size: 11px; padding: 1px 8px; border-radius: 8px; }
  .identical { color: #8b94a2; background: rgba(139, 148, 162, 0.12); }
  .different { color: #f0c850; background: rgba(240, 200, 80, 0.12); }
  .onlya { color: #ff5656; background: rgba(255, 86, 86, 0.12); }
  .onlyb { color: #50dc78; background: rgba(80, 220, 120, 0.12); }
  .error { color: #ff8585; background: rgba(255, 133, 133, 0.12); }
</style>
