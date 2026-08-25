<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open, save } from "@tauri-apps/plugin-dialog";

  let { pathA = $bindable(""), pathB = $bindable("") }: { pathA: string; pathB: string } = $props();

  // ---------- Unsigned patch (original) ----------
  let patchJson = $state("");
  let genError = $state("");
  let genLoading = $state(false);

  let oldPath = $state("");
  let patchPath = $state("");
  let outPath = $state("");
  let applyMsg = $state("");
  let applyError = $state("");
  let applyLoading = $state(false);

  // ---------- Signed patch ecosystem ----------
  interface KeyInfo { name: string; fingerprint_hex: string; has_private: boolean }
  interface PackedVerifyDto {
    ok: boolean; message: string; source_sha256: string; target_sha256: string;
    timestamp: number; engine_version: number; strategy: string; entry_count: number;
    signatures: { fingerprint: string; valid: boolean }[];
  }
  interface ApplyResult {
    ok: boolean; message: string; source_sha256: string; target_sha256: string; signed_by: string | null;
  }
  interface HistoryItem {
    kind: string; ts_ms: number; source: string; out: string;
    source_sha256: string; target_sha256: string; signed_by: string | null;
    ok: boolean; message: string;
  }

  let keys = $state<KeyInfo[]>([]);
  let signerName = $state("");
  let signStrategy = $state("0"); // 0=chunked 1=sliding 2=structural
  let packError = $state("");
  let packMsg = $state("");
  let packLoading = $state(false);

  let verifyPath = $state("");
  let verifyResult = $state<PackedVerifyDto | null>(null);
  let verifyError = $state("");
  let verifyLoading = $state(false);

  let pvPath = $state("");      // .rvapatch file
  let pvSrcPath = $state("");   // source file (apply)
  let pvOutPath = $state("");   // output (apply)
  let pvCurPath = $state("");   // current file (rollback)
  let pvRbOutPath = $state(""); // rollback output
  let pvApplyMsg = $state("");
  let pvApplyError = $state("");
  let pvLoading = $state(false);

  let history = $state<HistoryItem[]>([]);
  let historyError = $state("");

  const LS_PREFIX = "rva_patch_";
  function loadLS(key: string, fallback = "") {
    try { return localStorage.getItem(LS_PREFIX + key) ?? fallback; } catch { return fallback; }
  }
  function saveLS(key: string, val: string) {
    try { localStorage.setItem(LS_PREFIX + key, val); } catch { /* ignore */ }
  }

  onMount(async () => {
    // Restore last session progress (issue 4: no loss on re-entry)
    oldPath = loadLS("oldPath");
    patchPath = loadLS("patchPath");
    outPath = loadLS("outPath");
    verifyPath = loadLS("verifyPath");
    pvPath = loadLS("pvPath");
    pvSrcPath = loadLS("pvSrcPath");
    pvOutPath = loadLS("pvOutPath");
    pvCurPath = loadLS("pvCurPath");
    pvRbOutPath = loadLS("pvRbOutPath");
    signStrategy = loadLS("signStrategy", "0");
    if (loadLS("signerName")) signerName = loadLS("signerName");
    try {
      keys = await invoke<KeyInfo[]>("keystore_list");
      if (keys.length && !signerName) signerName = keys[0].name;
    } catch { /* silently ignore when the keystore is uninitialized */ }
    await loadHistory();
  });

  // Persist state on input changes
  $effect(() => {
    saveLS("oldPath", oldPath);
    saveLS("patchPath", patchPath);
    saveLS("outPath", outPath);
    saveLS("verifyPath", verifyPath);
    saveLS("pvPath", pvPath);
    saveLS("pvSrcPath", pvSrcPath);
    saveLS("pvOutPath", pvOutPath);
    saveLS("pvCurPath", pvCurPath);
    saveLS("pvRbOutPath", pvRbOutPath);
    saveLS("signStrategy", signStrategy);
    saveLS("signerName", signerName);
  });

  async function browsePathA() {
    const f = await open({ title: "选择 A（源文件）" });
    if (f && !Array.isArray(f)) pathA = f;
  }
  async function browsePathB() {
    const f = await open({ title: "选择 B（目标文件）" });
    if (f && !Array.isArray(f)) pathB = f;
  }

  async function refreshKeys() {
    try {
      keys = await invoke<KeyInfo[]>("keystore_list");
      if (keys.length && !keys.some((k) => k.name === signerName)) signerName = keys[0].name;
    } catch (e) { packError = String(e); }
  }
  async function loadHistory() {
    try {
      history = await invoke<HistoryItem[]>("patch_history");
    } catch (e) { historyError = String(e); }
  }

  async function generate() {
    if (!pathA || !pathB) return;
    genLoading = true;
    genError = "";
    try {
      patchJson = await invoke<string>("patch_generate", { pathA, pathB });
    } catch (e) {
      genError = String(e);
      patchJson = "";
    } finally {
      genLoading = false;
    }
  }

  async function savePatch() {
    if (!patchJson) return;
    const dst = await save({ title: "保存补丁", defaultPath: "rva_patch.rva" });
    if (!dst) return;
    try {
      await invoke("write_text_file", { path: dst, content: patchJson });
    } catch (e) {
      genError = String(e);
    }
  }

  async function browseOld() {
    const f = await open({ title: "选择原始文件" });
    if (f && !Array.isArray(f)) oldPath = f;
  }
  async function browsePatch() {
    const f = await open({ title: "选择补丁文件 (.rva)" });
    if (f && !Array.isArray(f)) patchPath = f;
  }
  async function browseOut() {
    const f = await save({ title: "选择输出文件", defaultPath: "patched.bin" });
    if (f) outPath = f;
  }

  async function apply() {
    if (!oldPath || !patchPath || !outPath) return;
    applyLoading = true;
    applyError = "";
    applyMsg = "";
    try {
      await invoke("patch_apply", { pathOld: oldPath, pathPatch: patchPath, pathOut: outPath });
      applyMsg = "补丁应用成功 → " + outPath;
    } catch (e) {
      applyError = String(e);
    } finally {
      applyLoading = false;
    }
  }

  // ---------- Signed patch: generate ----------
  async function packSign() {
    if (!pathA || !pathB || !signerName) return;
    packLoading = true;
    packError = "";
    packMsg = "";
    try {
      const bytes = await invoke<number[]>("patch_pack_sign", {
        pathA, pathB, strategy: Number(signStrategy), signerName,
      });
      const dst = await save({ title: "保存签名补丁", defaultPath: "rva_patch.rvapatch" });
      if (!dst) { packMsg = "签名补丁已生成（未保存）"; return; }
      await invoke("write_binary_file", { path: dst, data: bytes });
      packMsg = "签名补丁已保存 → " + dst;
    } catch (e) {
      packError = String(e);
    } finally {
      packLoading = false;
    }
  }

  // ---------- Signed patch: verify ----------
  async function browseVerify() {
    const f = await open({ title: "选择签名补丁 (.rvapatch)" });
    if (f && !Array.isArray(f)) verifyPath = f;
  }
  async function doVerify() {
    if (!verifyPath) return;
    verifyLoading = true;
    verifyError = "";
    verifyResult = null;
    try {
      const bytes = await invoke<number[]>("read_binary_file", { path: verifyPath });
      verifyResult = await invoke<PackedVerifyDto>("patch_pack_verify", { packed: bytes });
    } catch (e) {
      verifyError = String(e);
    } finally {
      verifyLoading = false;
    }
  }

  // ---------- Signed patch: apply / rollback ----------
  async function browsePv() {
    const f = await open({ title: "选择签名补丁 (.rvapatch)" });
    if (f && !Array.isArray(f)) pvPath = f;
  }
  async function browsePvSrc() {
    const f = await open({ title: "选择源文件" });
    if (f && !Array.isArray(f)) pvSrcPath = f;
  }
  async function browsePvOut() {
    const f = await save({ title: "选择输出文件", defaultPath: "patched.bin" });
    if (f) pvOutPath = f;
  }
  async function browsePvCur() {
    const f = await open({ title: "选择当前（已应用）文件" });
    if (f && !Array.isArray(f)) pvCurPath = f;
  }
  async function browsePvRbOut() {
    const f = await save({ title: "选择回滚输出文件", defaultPath: "restored.bin" });
    if (f) pvRbOutPath = f;
  }

  async function doApplyPacked() {
    if (!pvPath || !pvSrcPath || !pvOutPath) return;
    pvLoading = true;
    pvApplyError = "";
    pvApplyMsg = "";
    try {
      const bytes = await invoke<number[]>("read_binary_file", { path: pvPath });
      const res = await invoke<ApplyResult>("patch_apply_packed", {
        packed: bytes, sourcePath: pvSrcPath, outPath: pvOutPath,
      });
      pvApplyMsg = (res.ok ? "✅ " : "❌ ") + res.message + (res.signed_by ? `（签发者 ${res.signed_by.slice(0, 8)}…）` : "");
      await loadHistory();
    } catch (e) {
      pvApplyError = String(e);
    } finally {
      pvLoading = false;
    }
  }

  async function doRollbackPacked() {
    if (!pvPath || !pvCurPath || !pvRbOutPath) return;
    pvLoading = true;
    pvApplyError = "";
    pvApplyMsg = "";
    try {
      const bytes = await invoke<number[]>("read_binary_file", { path: pvPath });
      const res = await invoke<ApplyResult>("patch_rollback_packed", {
        packed: bytes, currentPath: pvCurPath, outPath: pvRbOutPath,
      });
      pvApplyMsg = (res.ok ? "✅ " : "❌ ") + res.message + (res.signed_by ? `（签发者 ${res.signed_by.slice(0, 8)}…）` : "");
      await loadHistory();
    } catch (e) {
      pvApplyError = String(e);
    } finally {
      pvLoading = false;
    }
  }

  function fmtTs(ms: number) {
    const d = new Date(ms);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  }
  function kindLabel(kind: string) {
    switch (kind) {
      case "apply": return "应用";
      case "apply_batch": return "批量应用";
      case "rollback": return "回滚";
      default: return kind;
    }
  }
</script>

<div class="view">
  <section class="block">
    <h3>生成补丁（A → B）</h3>
    <p class="hint">选择（或在比对页设定）A、B 文件生成差异补丁。</p>
    <div class="row">
      <input class="path-input grow" placeholder="A（源文件）" bind:value={pathA} />
      <button class="btn" onclick={browsePathA}>浏览</button>
      <span class="arr">→</span>
      <input class="path-input grow" placeholder="B（目标文件）" bind:value={pathB} />
      <button class="btn" onclick={browsePathB}>浏览</button>
    </div>
    <div class="row">
      <button class="btn primary" onclick={generate} disabled={genLoading || !pathA || !pathB}>
        {genLoading ? "生成中…" : "生成补丁"}
      </button>
      <button class="btn" onclick={savePatch} disabled={!patchJson}>保存补丁</button>
    </div>
    {#if genError}<div class="error">{genError}</div>{/if}
    {#if patchJson}<pre class="patch-preview">{patchJson}</pre>{/if}
  </section>

  <section class="block">
    <h3>应用补丁（未签名 .rva）</h3>
    <p class="hint">原始文件 + 补丁 → 输出新文件。</p>
    <div class="row">
      <input class="path-input grow" placeholder="原始文件" bind:value={oldPath} />
      <button class="btn" onclick={browseOld}>浏览</button>
    </div>
    <div class="row">
      <input class="path-input grow" placeholder="补丁文件 (.rva)" bind:value={patchPath} />
      <button class="btn" onclick={browsePatch}>浏览</button>
    </div>
    <div class="row">
      <input class="path-input grow" placeholder="输出文件" bind:value={outPath} />
      <button class="btn" onclick={browseOut}>浏览</button>
      <button class="btn primary" onclick={apply} disabled={applyLoading || !oldPath || !patchPath || !outPath}>
        {applyLoading ? "应用中…" : "应用补丁"}
      </button>
    </div>
    {#if applyError}<div class="error">{applyError}</div>{/if}
    {#if applyMsg}<div class="ok">{applyMsg}</div>{/if}
  </section>

  <section class="block">
    <h3>生成签名补丁（A → B，.rvapatch）</h3>
    <p class="hint">签名补丁容器含源/目标 SHA256、回滚快照与签发者签名，应用前强制校验。</p>
    <div class="row">
      <input class="path-input grow" placeholder="A（源文件）" bind:value={pathA} />
      <button class="btn" onclick={browsePathA}>浏览</button>
      <span class="arr">→</span>
      <input class="path-input grow" placeholder="B（目标文件）" bind:value={pathB} />
      <button class="btn" onclick={browsePathB}>浏览</button>
    </div>
    <div class="row">
      <label class="lbl" for="sign-strategy">比对策略</label>
      <select id="sign-strategy" class="sel" bind:value={signStrategy}>
        <option value="0">分块哈希（默认，最快）</option>
        <option value="1">滑动窗口（精确）</option>
        <option value="2">函数级结构（语义）</option>
      </select>
      <label class="lbl" for="sign-signer">签名者</label>
      <select id="sign-signer" class="sel" bind:value={signerName}>
        {#each keys as k}
          <option value={k.name}>{k.name} · {k.fingerprint_hex.slice(0, 8)}…</option>
        {/each}
      </select>
      <button class="btn" onclick={refreshKeys} title="刷新密钥列表">⟳</button>
      <button class="btn primary" onclick={packSign} disabled={packLoading || !pathA || !pathB || !signerName}>
        {packLoading ? "生成中…" : "生成签名补丁"}
      </button>
    </div>
    {#if packError}<div class="error">{packError}</div>{/if}
    {#if packMsg}<div class="ok">{packMsg}</div>{/if}
  </section>

  <section class="block">
    <h3>验证签名补丁</h3>
    <p class="hint">解析 .rvapatch 容器，校验源/目标 SHA256 与每条签名的有效性。</p>
    <div class="row">
      <input class="path-input grow" placeholder="签名补丁 (.rvapatch)" bind:value={verifyPath} />
      <button class="btn" onclick={browseVerify}>浏览</button>
      <button class="btn primary" onclick={doVerify} disabled={verifyLoading || !verifyPath}>
        {verifyLoading ? "验证中…" : "验证"}
      </button>
    </div>
    {#if verifyError}<div class="error">{verifyError}</div>{/if}
    {#if verifyResult}
      <div class="verify-box" class:verif-ok={verifyResult.ok} class:verif-bad={!verifyResult.ok}>
        <div class="verify-title">{verifyResult.ok ? "✅ 有效" : "❌ 无效"}：{verifyResult.message}</div>
        <div class="verify-meta">
          <div>策略：<span class="mono">{verifyResult.strategy}</span></div>
          <div>条目：<span class="mono">{verifyResult.entry_count}</span> · 引擎 v{verifyResult.engine_version}</div>
          <div>时间：<span class="mono">{fmtTs(verifyResult.timestamp * 1000)}</span></div>
          <div>源 SHA256：<span class="mono sha">{verifyResult.source_sha256}</span></div>
          <div>目标 SHA256：<span class="mono sha">{verifyResult.target_sha256}</span></div>
        </div>
        <div class="verify-sigs">
          {#each verifyResult.signatures as s}
            <div class="sig-row" class:sig-bad={!s.valid}>
              <span class="sig-ico">{s.valid ? "🟢" : "🔴"}</span>
              <span class="mono">{s.fingerprint}</span>
              <span>{s.valid ? "有效" : "无匹配可信公钥"}</span>
            </div>
          {:else}
            <div class="sig-row"><span class="mono">（未签名）</span></div>
          {/each}
        </div>
      </div>
    {/if}
  </section>

  <section class="block">
    <h3>应用 / 回滚签名补丁</h3>
    <p class="hint">应用：源文件 → 校验 SHA256 + 签名 → 输出目标文件。回滚：当前文件 → 校验 → 还原源文件。</p>
    <div class="row">
      <input class="path-input grow" placeholder="签名补丁 (.rvapatch)" bind:value={pvPath} />
      <button class="btn" onclick={browsePv}>浏览</button>
    </div>
    <div class="row">
      <input class="path-input grow" placeholder="源文件（应用用）" bind:value={pvSrcPath} />
      <button class="btn" onclick={browsePvSrc}>浏览</button>
      <input class="path-input grow" placeholder="输出文件（应用用）" bind:value={pvOutPath} />
      <button class="btn" onclick={browsePvOut}>浏览</button>
      <button class="btn primary" onclick={doApplyPacked} disabled={pvLoading || !pvPath || !pvSrcPath || !pvOutPath}>
        {pvLoading ? "处理中…" : "应用"}
      </button>
    </div>
    <div class="row">
      <input class="path-input grow" placeholder="当前文件（回滚用）" bind:value={pvCurPath} />
      <button class="btn" onclick={browsePvCur}>浏览</button>
      <input class="path-input grow" placeholder="回滚输出文件" bind:value={pvRbOutPath} />
      <button class="btn" onclick={browsePvRbOut}>浏览</button>
      <button class="btn warn" onclick={doRollbackPacked} disabled={pvLoading || !pvPath || !pvCurPath || !pvRbOutPath}>
        回滚
      </button>
    </div>
    {#if pvApplyError}<div class="error">{pvApplyError}</div>{/if}
    {#if pvApplyMsg}<div class="ok">{pvApplyMsg}</div>{/if}
  </section>

  <section class="block">
    <h3>历史记录</h3>
    <div class="row">
      <button class="btn" onclick={loadHistory}>刷新</button>
    </div>
    {#if historyError}<div class="error">{historyError}</div>{/if}
    <table class="hist-table">
      <thead>
        <tr><th>时间</th><th>类型</th><th>源文件</th><th>输出</th><th>签发者</th><th>状态</th></tr>
      </thead>
      <tbody>
        {#each history as h}
          <tr>
            <td class="mono nowrap">{fmtTs(h.ts_ms)}</td>
            <td>{kindLabel(h.kind)}</td>
            <td class="ellip" title={h.source}>{h.source}</td>
            <td class="ellip" title={h.out}>{h.out}</td>
            <td class="mono">{h.signed_by ? h.signed_by.slice(0, 8) + "…" : "—"}</td>
            <td>
              {#if h.ok}<span class="ok-txt">✅ {h.message}</span>{:else}<span class="bad-txt">❌ {h.message}</span>{/if}
            </td>
          </tr>
        {:else}
          <tr><td colspan="6" class="empty">暂无记录</td></tr>
        {/each}
      </tbody>
    </table>
  </section>
</div>

<style>
  /* Fix: the global .view is overflow:hidden; this page has lots of content and needs to scroll (issue 3) */
  .view { overflow-y: auto; overflow-x: hidden; }
  .block { padding: 16px 20px; border-bottom: 1px solid #232830; }
  h3 { margin: 0 0 4px; font-size: 14px; color: #d6dae0; font-weight: 600; }
  .hint { margin: 0 0 12px; font-size: 12px; color: #6b7482; }
  .row { display: flex; align-items: center; gap: 10px; margin: 8px 0; }
  .arr { color: #9b7dff; }
  .ellip { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .patch-preview { margin: 10px 0 0; padding: 12px; background: #0d0f12; border: 1px solid #232830; border-radius: 6px; color: #a8d1ff; font-family: Consolas, monospace; font-size: 12px; max-height: 240px; overflow: auto; white-space: pre-wrap; word-break: break-all; }
  .ok { margin: 8px 0; padding: 8px 12px; background: rgba(80, 220, 120, 0.1); border: 1px solid rgba(80, 220, 120, 0.3); border-radius: 6px; color: #50dc78; font-size: 13px; }
  .error { margin: 8px 0; padding: 8px 12px; background: rgba(255, 133, 133, 0.08); border: 1px solid rgba(255, 133, 133, 0.3); border-radius: 6px; color: #ff8585; font-size: 13px; }
  .mono { font-family: Consolas, monospace; font-size: 12px; color: #8b94a2; }
  .lbl { font-size: 12px; color: #8b94a2; white-space: nowrap; }
  .sel { height: 32px; padding: 0 8px; border-radius: 6px; border: 1px solid #2a313a; background: #1c2128; color: #d6dae0; font-size: 12px; }
  .btn.warn { border-color: #e6b450; color: #e6b450; }
  .verify-box { margin: 10px 0 0; padding: 12px 14px; border-radius: 8px; border: 1px solid; font-size: 12px; }
  .verify-box.verif-ok { background: rgba(80, 220, 120, 0.06); border-color: rgba(80, 220, 120, 0.35); }
  .verify-box.verif-bad { background: rgba(255, 133, 133, 0.06); border-color: rgba(255, 133, 133, 0.35); }
  .verify-title { font-size: 13px; font-weight: 600; color: #e8ebef; margin-bottom: 8px; }
  .verify-meta { display: flex; flex-direction: column; gap: 4px; color: #b8c2cc; }
  .verify-meta .sha { word-break: break-all; color: #a8d1ff; }
  .verify-sigs { margin-top: 10px; display: flex; flex-direction: column; gap: 4px; }
  .sig-row { display: flex; align-items: center; gap: 8px; color: #b8c2cc; }
  .sig-row.sig-bad { color: #ff8585; }
  .sig-ico { font-size: 11px; }
  .hist-table { width: 100%; border-collapse: collapse; margin-top: 8px; font-size: 12px; }
  .hist-table th { text-align: left; color: #6b7482; font-weight: 500; padding: 6px 8px; border-bottom: 1px solid #232830; white-space: nowrap; }
  .hist-table td { padding: 6px 8px; border-bottom: 1px solid #1a1f26; color: #b8c2cc; vertical-align: top; }
  .hist-table td.ellip { max-width: 200px; }
  .nowrap { white-space: nowrap; }
  .empty { text-align: center; color: #5b6472; padding: 20px 0 !important; }
  .ok-txt { color: #50dc78; }
  .bad-txt { color: #ff8585; }
</style>
