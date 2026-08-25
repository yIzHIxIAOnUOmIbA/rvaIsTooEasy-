<script lang="ts">
  // Settings view (peer of the main view's left nav): left panel nav + right content;
  // Changes take effect immediately and persist via the global settings store.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { settings, setSetting, resetGroup, type SettingKey, type CopyFormat } from "./settings.svelte";

  declare const __APP_VERSION__: string;

  // Show only the bare version, stripping the build timestamp (the timestamp is used only by the HUD to tell old/new builds apart)
  const APP_VER = __APP_VERSION__.split("-b")[0];

  type PanelId = "display" | "diff" | "debug" | "keys" | "keystore" | "about";

  let panel = $state<PanelId>("display");
  let bprOpen = $state(false);
  let cfOpen = $state(false);

  const COPY_FORMAT_LABELS: Record<CopyFormat, string> = {
    hexsp: "Hex 空格",
    hex: "Hex 紧凑",
    carr: "C 数组",
    rarr: "Rust 数组",
    py: "Python bytes",
    ascii: "ASCII",
  };
  let toastVisible = $state(false);
  let toastMsg = $state("已恢复默认");
  let toastSeq = $state(0);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  const PANEL_TITLE: Record<PanelId, string> = {
    display: "显示", diff: "比对", debug: "调试", keys: "快捷键", keystore: "密钥库", about: "关于",
  };
  const NAV: { id: PanelId; label: string }[] = [
    { id: "display", label: "显示" }, { id: "diff", label: "比对" },
    { id: "debug", label: "调试" }, { id: "keys", label: "快捷键" },
    { id: "keystore", label: "密钥库" }, { id: "about", label: "关于" },
  ];

  // ---------- Key store ----------
  interface KeyInfo { name: string; fingerprint_hex: string; has_private: boolean }
  let keys = $state<KeyInfo[]>([]);
  let keyError = $state("");
  let keyMsg = $state("");
  let newKeyName = $state("");
  let importName = $state("");
  let importPub = $state("");
  let busy = $state(false);
  let exportedPub = $state("");
  let exportedName = $state("");

  async function refreshKeys() {
    try {
      keys = await invoke<KeyInfo[]>("keystore_list");
    } catch (e) {
      keyError = String(e);
    }
  }
  async function doGenerate() {
    const name = newKeyName.trim();
    if (!name) return;
    busy = true; keyError = ""; keyMsg = "";
    try {
      await invoke("keystore_generate", { name });
      newKeyName = "";
      keyMsg = `密钥 ${name} 已生成（私钥已加密存储）`;
      await refreshKeys();
    } catch (e) {
      keyError = String(e);
    } finally {
      busy = false;
    }
  }
  async function doDelete(name: string) {
    busy = true; keyError = ""; keyMsg = "";
    try {
      await invoke("keystore_delete", { name });
      keyMsg = `密钥 ${name} 已删除`;
      if (exportedName === name) { exportedPub = ""; exportedName = ""; }
      await refreshKeys();
    } catch (e) {
      keyError = String(e);
    } finally {
      busy = false;
    }
  }
  async function doExport(name: string) {
    busy = true; keyError = "";
    try {
      exportedPub = await invoke<string>("keystore_export_pub", { name });
      exportedName = name;
    } catch (e) {
      keyError = String(e);
    } finally {
      busy = false;
    }
  }
  async function doImport() {
    const name = importName.trim();
    const hex = importPub.trim();
    if (!name || !hex) return;
    busy = true; keyError = ""; keyMsg = "";
    try {
      await invoke("keystore_import_pub", { name, pubHex: hex });
      importName = ""; importPub = "";
      keyMsg = `公钥 ${name} 已导入（仅信任，无私钥）`;
      await refreshKeys();
    } catch (e) {
      keyError = String(e);
    } finally {
      busy = false;
    }
  }
  async function copyPub() {
    try {
      await navigator.clipboard.writeText(exportedPub);
      showToast("公钥已复制");
    } catch (e) {
      keyError = String(e);
    }
  }
  const BYTES_OPTIONS = [8, 12, 16, 20, 24, 28, 32];
  const SHORTCUTS: { op: string; keys: string }[] = [
    { op: "打开文件", keys: "Ctrl+O" }, { op: "撤销", keys: "Ctrl+Z" },
    { op: "上一个差异", keys: "Prev" }, { op: "下一个差异", keys: "Next" },
    { op: "跳转偏移", keys: "Go" }, { op: "搜索字节序列", keys: "Ctrl+F" }, { op: "复制选中", keys: "Ctrl+C" },
  ];
  const ENGINE_STATS: { label: string; value: string }[] = [
    { label: "sliding 吞吐", value: "4.0 MB/s" },
    { label: "chunked 吞吐", value: "554 MB/s" },
    { label: "sliding 地址漂移 F1", value: "1.000" },
    { label: "补丁往返验证", value: "4/4 PASS" },
  ];

  onMount(() => {
    const close = () => (bprOpen = false);
    document.addEventListener("click", close);
    refreshKeys();
    return () => document.removeEventListener("click", close);
  });

  function setBool(key: SettingKey, on: boolean) {
    setSetting(key, on as never);
  }
  function clampNum(key: SettingKey, v: number, min: number, max: number) {
    setSetting(key, Math.min(max, Math.max(min, Math.round(v))) as never);
  }
  function showToast(msg: string) {
    clearTimeout(toastTimer);
    toastMsg = msg;
    toastSeq++;
    toastVisible = true;
    toastTimer = setTimeout(() => (toastVisible = false), 1500);
  }
  function doReset(gid: string) {
    resetGroup(gid);
    showToast("已恢复默认");
  }
  async function copyFingerprint(hex: string) {
    try {
      await navigator.clipboard.writeText(hex);
      showToast("指纹已复制");
    } catch (e) {
      keyError = String(e);
    }
  }
</script>

<div class="view">
  <div class="viewbar">
    <span class="title">设置</span>
    <span class="current-panel">当前面板：{PANEL_TITLE[panel]}</span>
  </div>

  <div class="body">
    <nav class="sidebar">
      <div class="nav-group">
        {#each NAV as n}
          <button class="nav-item" class:active={panel === n.id} onclick={() => (panel = n.id)}>
            <span class="bar"></span>{n.label}
          </button>
        {/each}
      </div>
    </nav>

    <main class="content" key={panel}>
      {#if toastVisible}
        {#key toastSeq}
          <div class="toast">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5" /></svg>
            {toastMsg}
          </div>
        {/key}
      {/if}

      <!-- Display panel -->
      {#if panel === "display"}
        <h2 class="panel-title">显示</h2>
        <p class="panel-desc">控制十六进制视图的排版与可读性，所有变更即时生效。</p>
        <div class="items">
          <div class="row">
            <div class="lab"><span class="name">每行字节数</span><span class="hint">8 – 32，默认 16</span></div>
            <div class="ctl">
              <div class="select-wrap" onclick={(e) => e.stopPropagation()}>
                <button class="select-btn" onclick={() => (bprOpen = !bprOpen)} aria-haspopup="listbox" aria-expanded={bprOpen}>
                  <span>{settings.bytesPerRow}</span>
                  <svg class="chev" class:open={bprOpen} width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6" /></svg>
                </button>
                {#if bprOpen}
                  <div class="select-list" role="listbox">
                    {#each BYTES_OPTIONS as v}
                      <button class="option" class:sel={settings.bytesPerRow === v} role="option" aria-selected={settings.bytesPerRow === v} onclick={() => { setSetting("bytesPerRow", v as never); bprOpen = false; }}>{v}</button>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">十六进制字体大小</span><span class="hint">12 – 18px，默认 14px</span></div>
            <div class="ctl">
              <input class="range" type="range" min="12" max="18" step="1" value={settings.hexFontSize} style={`--fill: ${((settings.hexFontSize - 12) / 6) * 100}%`} oninput={(e) => setSetting("hexFontSize", Number((e.currentTarget as HTMLInputElement).value) as never)} />
              <span class="val">{settings.hexFontSize}px</span>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">显示 ASCII 侧栏</span><span class="hint">每行字节右侧展示可打印字符</span></div>
            <div class="ctl">
              <button class="switch" class:on={settings.showAscii} role="switch" aria-checked={settings.showAscii} onclick={() => setBool("showAscii", !settings.showAscii)}><span class="knob"></span></button>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">显示行号 gutter</span><span class="hint">左侧地址列，双击整行可选中</span></div>
            <div class="ctl">
              <button class="switch" class:on={settings.showGutter} role="switch" aria-checked={settings.showGutter} onclick={() => setBool("showGutter", !settings.showGutter)}><span class="knob"></span></button>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">地址进制</span><span class="hint">行号 gutter 偏移量显示格式</span></div>
            <div class="ctl">
              <div class="seg">
                <button class:on={settings.addrBase === "hex"} onclick={() => setSetting("addrBase", "hex" as never)}>HEX</button>
                <button class:on={settings.addrBase === "dec"} onclick={() => setSetting("addrBase", "dec" as never)}>DEC</button>
              </div>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">十六进制大小写</span><span class="hint">开启显示 Upper，关闭显示 Lower</span></div>
            <div class="ctl">
              <button class="switch" class:on={settings.hexCase === "upper"} role="switch" aria-checked={settings.hexCase === "upper"} onclick={() => setSetting("hexCase", (settings.hexCase === "upper" ? "lower" : "upper") as never)}><span class="knob"></span></button>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">复制格式</span><span class="hint">Ctrl+C / 右键复制 / 工具条复制统一使用</span></div>
            <div class="ctl">
              <div class="select-wrap" onclick={(e) => e.stopPropagation()}>
                <button class="select-btn" onclick={() => (cfOpen = !cfOpen)} aria-haspopup="listbox" aria-expanded={cfOpen}>
                  <span>{COPY_FORMAT_LABELS[settings.copyFormat]}</span>
                  <svg class="chev" class:open={cfOpen} width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6" /></svg>
                </button>
                {#if cfOpen}
                  <div class="select-list" role="listbox">
                    {#each Object.entries(COPY_FORMAT_LABELS) as [k, label]}
                      <button class="option" class:sel={settings.copyFormat === k} role="option" aria-selected={settings.copyFormat === k} onclick={() => { setSetting("copyFormat", k as never); cfOpen = false; }}>{label}</button>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          </div>
        </div>
        <button class="reset-btn" onclick={() => doReset("display")}>恢复默认</button>
      {/if}

      <!-- Diff panel -->
      {#if panel === "diff"}
        <h2 class="panel-title">比对</h2>
        <p class="panel-desc">控制差异比对引擎的默认行为。</p>
        <div class="items">
          <div class="row">
            <div class="lab"><span class="name">默认策略</span><span class="hint">主视图比对时使用的引擎策略</span></div>
            <div class="ctl">
              <div class="strategy-cards">
                {#each ["sliding", "structural", "chunked"] as s}
                  <button class="scard" class:on={settings.diffStrategy === s} onclick={() => setSetting("diffStrategy", s as never)}>{s}</button>
                {/each}
              </div>
            </div>
          </div>

          {#if settings.diffStrategy === "sliding"}
            <div class="row reveal">
              <div class="lab"><span class="name">sliding 窗口大小</span><span class="hint">4 – 64，默认 16</span></div>
              <div class="ctl">
                <input class="num" type="number" min="4" max="64" step="1" value={settings.slidingWindow} onchange={(e) => clampNum("slidingWindow", Number((e.currentTarget as HTMLInputElement).value), 4, 64)} />
                <span class="unit">bytes</span>
              </div>
            </div>
          {/if}

          <div class="row">
            <div class="lab"><span class="name">比对后自动跳转首个差异</span><span class="hint">比对完成立即滚动到第一个差异位置</span></div>
            <div class="ctl">
              <button class="switch" class:on={settings.autoJumpFirst} role="switch" aria-checked={settings.autoJumpFirst} onclick={() => setBool("autoJumpFirst", !settings.autoJumpFirst)}><span class="knob"></span></button>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">差异徽章悬停延迟</span><span class="hint">0 – 1000ms，默认 300ms</span></div>
            <div class="ctl">
              <input class="num" type="number" min="0" max="1000" step="50" value={settings.badgeHoverDelay} onchange={(e) => clampNum("badgeHoverDelay", Number((e.currentTarget as HTMLInputElement).value), 0, 1000)} />
              <span class="unit">ms</span>
            </div>
          </div>
        </div>
        <button class="reset-btn" onclick={() => doReset("diff")}>恢复默认</button>
      {/if}

      <!-- Debug panel -->
      {#if panel === "debug"}
        <h2 class="panel-title">调试</h2>
        <p class="panel-desc">面向开发者的诊断开关。</p>
        <div class="items">
          <div class="row">
            <div class="lab"><span class="name">HUD 调试开关</span><span class="hint">显示性能与差异统计浮层</span></div>
            <div class="ctl">
              <button class="switch" class:on={settings.showHud} role="switch" aria-checked={settings.showHud} onclick={() => setBool("showHud", !settings.showHud)}><span class="knob"></span></button>
            </div>
          </div>
          <div class="row">
            <div class="lab"><span class="name">输出引擎日志到控制台</span><span class="hint">将引擎执行日志输出至开发者控制台</span></div>
            <div class="ctl">
              <button class="switch" class:on={settings.engineLog} role="switch" aria-checked={settings.engineLog} onclick={() => setBool("engineLog", !settings.engineLog)}><span class="knob"></span></button>
            </div>
          </div>
        </div>
        <button class="reset-btn" onclick={() => doReset("debug")}>恢复默认</button>
      {/if}

      <!-- Shortcuts panel -->
      {#if panel === "keys"}
        <h2 class="panel-title">快捷键</h2>
        <p class="panel-desc">主视图中可用的键盘操作。</p>
        <table class="keys-table">
          <thead><tr><th>操作</th><th class="right">快捷键</th></tr></thead>
          <tbody>
            {#each SHORTCUTS as s}
              <tr><td>{s.op}</td><td class="right"><kbd>{s.keys}</kbd></td></tr>
            {/each}
          </tbody>
        </table>
      {/if}

      <!-- Key store panel -->
      {#if panel === "keystore"}
        <h2 class="panel-title">密钥库</h2>
        <p class="panel-desc">签名密钥管理：生成密钥对用于补丁签名与验签。私钥经系统密钥环加密存储，公钥可导出分发建立信任。</p>

        <div class="items">
          <div class="row">
            <div class="lab"><span class="name">生成密钥对</span><span class="hint">本地生成 Ed25519 密钥对，私钥加密入库</span></div>
            <div class="ctl">
              <input class="num key-name" type="text" placeholder="密钥名称" bind:value={newKeyName} />
              <button class="mini-btn primary" onclick={doGenerate} disabled={busy || !newKeyName.trim()}>生成</button>
            </div>
          </div>

          <div class="row">
            <div class="lab"><span class="name">导入公钥</span><span class="hint">从其它机器导入公钥（64 位十六进制），用于验证其签名</span></div>
            <div class="ctl">
              <input class="num key-name" type="text" placeholder="名称" bind:value={importName} />
              <input class="num key-pub" type="text" placeholder="公钥 hex (64)" bind:value={importPub} spellcheck={false} />
              <button class="mini-btn primary" onclick={doImport} disabled={busy || !importName.trim() || !importPub.trim()}>导入</button>
            </div>
          </div>
        </div>

        {#if keyError}<div class="key-msg err">{keyError}</div>{/if}
        {#if keyMsg}<div class="key-msg ok">{keyMsg}</div>{/if}

        <div class="key-list">
          {#each keys as k}
            <div class="key-item">
              <div class="key-left">
                <span class="key-name-text">{k.name}</span>
                <span class="key-fp mono">{k.fingerprint_hex}</span>
              </div>
              <div class="key-right">
                <span class="key-badge" class:priv={k.has_private}>{k.has_private ? "私钥" : "仅公钥"}</span>
                <button class="mini-btn" onclick={() => copyFingerprint(k.fingerprint_hex)}>复制指纹</button>
                <button class="mini-btn" onclick={() => doExport(k.name)} disabled={busy}>导出</button>
                <button class="mini-btn danger" onclick={() => doDelete(k.name)} disabled={busy}>删除</button>
              </div>
            </div>
          {:else}
            <div class="key-empty">密钥库为空。生成一个密钥对或导入信任公钥。</div>
          {/each}
        </div>

        {#if exportedPub}
          <div class="export-box">
            <div class="export-head">
              <span class="mono">{exportedName} 公钥</span>
              <button class="mini-btn" onclick={copyPub}>复制</button>
            </div>
            <pre class="export-hex">{exportedPub}</pre>
          </div>
        {/if}
      {/if}

      <!-- About panel -->
      {#if panel === "about"}
        <h2 class="panel-title">关于</h2>
        <p class="panel-desc">应用信息与引擎能力摘要。</p>
        <div class="about-wrap">
          <div class="app-name">RVA Compare</div>
          <div class="app-ver">v{APP_VER}</div>
          <div class="engine-box">
            {#each ENGINE_STATS as s}
              <div class="stat"><span class="lab">{s.label}</span><span class="val">{s.value}</span></div>
            {/each}
          </div>
          <a class="repo" href="https://github.com/rvaIsTooEasy" target="_blank" rel="noreferrer">rvaIsTooEasy · GitHub</a>
          <p class="license">RVA Compare 基于 Rust / Tauri / Svelte 构建。差异比对引擎 rva_core 以 MIT 许可证开源发布；前端界面与交互设计版权归本项目所有。本工具仅供安全研究与学习使用。</p>
        </div>
      {/if}
    </main>
  </div>
</div>

<style>
  * { box-sizing: border-box; }
  /* -- Main two-column layout -- */
  .body { display: flex; flex: 1; min-height: 0; }
  .sidebar {
    width: 168px; flex-shrink: 0; background: #12151a; border-right: 1px solid #232830;
    padding: 16px 8px;
  }
  .nav-group { display: flex; flex-direction: column; gap: 4px; }
  .nav-item {
    position: relative; display: flex; align-items: center; height: 38px; width: 100%;
    border-radius: 6px; background: transparent; border: none; padding: 0 16px;
    font-size: 13px; font-weight: 500; color: #8b94a2; text-align: left; cursor: pointer;
    transition: background 150ms, color 150ms;
  }
  .nav-item:hover { background: #1d232b; color: #c3cad3; }
  .nav-item.active { background: #243041; color: #e8ebef; }
  .nav-item .bar {
    position: absolute; left: 0; top: 50%; transform: translateY(-50%);
    width: 3px; height: 22px; border-radius: 0 2px 2px 0; background: #9b7dff; opacity: 0;
  }
  .nav-item.active .bar { opacity: 1; }
  .current-panel { font-size: 12px; color: #5b6472; margin-left: 8px; }
  /* -- Right content area -- */
  .content {
    flex: 1; padding: 28px 32px; overflow-y: auto; max-width: 720px;
    scrollbar-width: thin; scrollbar-color: #2a313a #12151a;
  }
  .content::-webkit-scrollbar { width: 6px; }
  .content::-webkit-scrollbar-track { background: transparent; }
  .content::-webkit-scrollbar-thumb { background: #2a313a; border-radius: 3px; transition: background 150ms; }
  .content::-webkit-scrollbar-thumb:hover { background: #343d48; }
  .panel-title { margin: 0; font-size: 20px; font-weight: 600; color: #e8ebef; }
  .panel-desc { margin: 8px 0 0; font-size: 13px; color: #8b94a2; line-height: 1.4; max-width: 560px; }
  .items { margin-top: 24px; }
  .row {
    display: flex; justify-content: space-between; align-items: center;
    min-height: 56px; padding: 14px 0; border-bottom: 1px solid rgba(35,40,48,0.5);
  }
  .row:last-child { border-bottom: none; }
  .lab { display: flex; flex-direction: column; gap: 4px; max-width: 400px; }
  .lab .name { font-size: 14px; font-weight: 500; color: #e8ebef; }
  .lab .hint { font-size: 12px; color: #6a7480; line-height: 1.4; }
  .ctl { display: flex; align-items: center; justify-content: flex-end; gap: 24px; flex-shrink: 0; }
  .reveal { animation: reveal 200ms ease-out; }
  @keyframes reveal { from { height: 0; opacity: 0; } to { height: 56px; opacity: 1; } }
  /* -- Reset to defaults -- */
  .reset-btn {
    margin-top: 24px; margin-left: auto; display: block;
    width: 100px; height: 32px; border-radius: 6px; background: transparent;
    border: 1px solid #2a313a; font-size: 12px; font-weight: 500; color: #8b94a2; cursor: pointer;
    transition: border-color 150ms, color 150ms;
  }
  .reset-btn:hover { border-color: #ff8585; color: #ff8585; }
  .toast {
    position: fixed; top: 72px; left: 50%; transform: translateX(-50%);
    display: flex; align-items: center; gap: 6px; height: 32px; padding: 0 14px;
    background: rgba(74,222,128,0.1); border: 1px solid #4ade80; border-radius: 6px;
    color: #b8c2cc; font-size: 12px; z-index: 50;
    animation: toastin 200ms ease-out;
  }
  @keyframes toastin { from { opacity: 0; transform: translateX(-50%) translateY(-8px); } to { opacity: 1; transform: translateX(-50%) translateY(0); } }
  .toast svg { color: #4ade80; }
  /* -- Dropdown (bytes per row) -- */
  .select-wrap { position: relative; }
  .select-btn {
    display: flex; align-items: center; justify-content: space-between;
    width: 120px; height: 32px; border-radius: 6px; background: #1c2128;
    border: 1px solid #2a313a; color: #e8ebef; font-size: 13px; padding: 0 12px; cursor: pointer;
    transition: border-color 150ms;
  }
  .select-btn:hover { border-color: #343d48; }
  .chev { color: #8b94a2; transition: transform 200ms cubic-bezier(0.4, 0, 0.2, 1); }
  .chev.open { transform: rotate(180deg); }
  .select-list {
    position: absolute; top: 40px; right: 0; width: 120px; background: #1c2128;
    border: 1px solid #2a313a; border-radius: 6px; box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    max-height: 240px; overflow-y: auto; z-index: 30; padding: 4px;
    animation: dropdown 150ms ease-out;
  }
  @keyframes dropdown { from { opacity: 0; transform: translateY(-4px); } to { opacity: 1; transform: translateY(0); } }
  .option {
    display: block; width: 100%; height: 32px; text-align: left; padding: 0 12px;
    border-radius: 4px; background: transparent; border: none; color: #e8ebef; font-size: 13px; cursor: pointer;
    transition: background 100ms;
  }
  .option:hover { background: #243041; }
  .option.sel { background: rgba(155,125,255,0.15); color: #9b7dff; }

  /* -- Slider (hex font size) -- */
  .range {
    width: 180px; height: 4px; appearance: none; -webkit-appearance: none; background: transparent; cursor: pointer;
  }
  .range::-webkit-slider-runnable-track {
    height: 4px; border-radius: 2px;
    background: linear-gradient(to right, #9b7dff 0%, #9b7dff var(--fill, 50%), #252a32 var(--fill, 50%), #252a32 100%);
  }
  .range::-moz-range-track { height: 4px; border-radius: 2px; background: #2a313a; }
  .range::-moz-range-progress { height: 4px; border-radius: 2px; background: #9b7dff; }
  .range::-webkit-slider-thumb {
    appearance: none; -webkit-appearance: none; width: 16px; height: 16px; border-radius: 50%;
    background: #e8e2f8; border: 2px solid #9b7dff; box-shadow: 0 2px 6px rgba(0,0,0,0.3);
    margin-top: -6px; transition: transform 300ms cubic-bezier(0.4,0,0.2,1);
  }
  .range::-moz-range-thumb {
    width: 16px; height: 16px; border-radius: 50%; background: #e8e2f8;
    border: 2px solid #9b7dff; box-shadow: 0 2px 6px rgba(0,0,0,0.3);
  }
  .range:active::-webkit-slider-thumb { transform: scale(1.15); }
  .val { width: 40px; font-size: 13px; color: #b8c2cc; text-align: left; }

  /* —— toggle switch —— */
  .switch {
    position: relative; width: 44px; height: 24px; border-radius: 12px; border: none; padding: 0;
    background: #2a313a; cursor: pointer; transition: background 200ms ease-in-out; flex-shrink: 0;
  }
  .switch.on { background: #4ade80; box-shadow: 0 0 12px rgba(74,222,128,0.4); }
  .switch .knob {
    position: absolute; top: 3px; left: 3px; width: 18px; height: 18px; border-radius: 50%;
    background: #ffffff; transition: left 200ms cubic-bezier(0.4,0,0.2,1);
  }
  .switch.on .knob { left: 23px; }

  /* -- Segmented button group (address base) -- */
  .seg {
    display: flex; width: 132px; height: 32px; border-radius: 6px;
    background: #1c2128; padding: 3px; gap: 2px;
  }
  .seg button {
    flex: 1; border-radius: 4px; border: none; background: transparent;
    font-size: 13px; font-weight: 500; color: #8b94a2; cursor: pointer; transition: background 150ms, color 150ms;
  }
  .seg button.on { background: #8a6fce; color: #f5f0ff; }

  /* -- Strategy cards -- */
  .strategy-cards { display: flex; width: 312px; height: 48px; gap: 8px; }
  .scard {
    position: relative; flex: 1; height: 48px; border-radius: 8px; background: #1c2128;
    border: 1px solid #2a313a; font-size: 13px; font-weight: 500; color: #8b94a2; cursor: pointer;
    transition: border-color 150ms, color 150ms, background 150ms;
  }
  .scard:hover { color: #e8ebef; }
  .scard.on { border-color: #9b7dff; background: rgba(155,125,255,0.08); color: #f0edf8; }
  .scard.on::before {
    content: ""; position: absolute; left: 0; top: 50%; transform: translateY(-50%);
    width: 3px; height: 32px; border-radius: 0 2px 2px 0; background: #9b7dff;
    animation: barin 200ms;
  }
  @keyframes barin { from { opacity: 0; } to { opacity: 1; } }

  /* -- Number input -- */
  .num {
    width: 80px; height: 32px; border-radius: 6px; background: #1c2128;
    border: 1px solid #2a313a; color: #e8ebef; font-size: 13px; padding: 0 12px;
    transition: border-color 150ms;
  }
  .num:hover { border-color: #343d48; }
  .num:focus { outline: none; border-color: #9b7dff; }
  .unit { font-size: 11px; color: #6a7480; margin-left: 4px; }

  /* -- Shortcuts table -- */
  .keys-table { width: 100%; border-collapse: collapse; margin-top: 24px; }
  .keys-table thead th {
    height: 36px; background: #12151a; font-size: 12px; font-weight: 600; color: #8b94a2;
    text-align: left; padding-left: 12px; vertical-align: middle;
  }
  .keys-table thead th.right, .keys-table td.right { text-align: right; padding-right: 12px; }
  .keys-table tbody td {
    height: 40px; font-size: 13px; color: #e8ebef; vertical-align: middle; padding-left: 12px;
    border-bottom: 1px solid rgba(35,40,48,0.5); transition: background 150ms;
  }
  .keys-table tbody tr:hover td { background: rgba(155,125,255,0.05); }
  .keys-table kbd {
    background: #2a313a; border: 1px solid #343d48; border-radius: 4px; padding: 2px 6px;
    font-family: Consolas, monospace; font-size: 11px; font-weight: 500; color: #b8c2cc;
  }

  /* -- Key store -- */
  .key-name { width: 140px; }
  .key-pub { width: 220px; font-family: Consolas, monospace; font-size: 12px; }
  .mini-btn {
    height: 32px; padding: 0 14px; border-radius: 6px; border: 1px solid #2a313a;
    background: #1c2128; color: #b8c2cc; font-size: 12px; font-weight: 500; cursor: pointer;
    transition: border-color 150ms, color 150ms, background 150ms; white-space: nowrap;
  }
  .mini-btn:hover { border-color: #343d48; color: #e8ebef; }
  .mini-btn.primary { border-color: #9b7dff; color: #cdb9ff; background: rgba(155,125,255,0.08); }
  .mini-btn.primary:hover { background: rgba(155,125,255,0.16); }
  .mini-btn.danger:hover { border-color: #ff8585; color: #ff8585; }
  .mini-btn:disabled { opacity: 0.45; cursor: not-allowed; }
  .key-msg { margin-top: 12px; padding: 8px 12px; border-radius: 6px; font-size: 12px; }
  .key-msg.err { background: rgba(255,133,133,0.08); border: 1px solid rgba(255,133,133,0.3); color: #ff8585; }
  .key-msg.ok { background: rgba(80,220,120,0.08); border: 1px solid rgba(80,220,120,0.3); color: #50dc78; }
  .key-list { margin-top: 20px; display: flex; flex-direction: column; gap: 8px; }
  .key-item {
    display: flex; justify-content: space-between; align-items: center; gap: 12px;
    padding: 12px 14px; background: #12151a; border: 1px solid #232830; border-radius: 8px;
    transition: border-color 150ms;
  }
  .key-item:hover { border-color: #2a313a; }
  .key-left { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .key-name-text { font-size: 14px; font-weight: 600; color: #e8ebef; }
  .key-fp { font-size: 12px; color: #6a7480; word-break: break-all; }
  .key-right { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }
  .key-badge {
    height: 22px; padding: 0 8px; border-radius: 11px; background: #1c2128;
    border: 1px solid #2a313a; color: #8b94a2; font-size: 11px; line-height: 20px;
  }
  .key-badge.priv { color: #50dc78; border-color: rgba(80,220,120,0.35); background: rgba(80,220,120,0.08); }
  .key-empty { margin-top: 20px; padding: 24px; text-align: center; font-size: 12px; color: #5b6472; border: 1px dashed #232830; border-radius: 8px; }
  .export-box { margin-top: 16px; background: #12151a; border: 1px solid #232830; border-radius: 8px; padding: 12px 14px; }
  .export-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; }
  .export-head .mono { font-size: 12px; color: #b8c2cc; }
  .export-hex { margin: 0; font-family: Consolas, monospace; font-size: 12px; color: #a8d1ff; white-space: pre-wrap; word-break: break-all; line-height: 1.5; }

  /* -- About -- */
  .about-wrap { margin-top: 24px; }
  .app-name { font-size: 24px; font-weight: 600; color: #e8ebef; }
  .app-ver { margin-top: 8px; font-size: 13px; color: #8b94a2; }
  .engine-box {
    margin-top: 24px; background: #0e1013; border: 1px solid #232830;
    border-radius: 8px; padding: 16px; width: 100%;
  }
  .stat { display: flex; align-items: center; gap: 12px; min-height: 24px; padding: 2px 0; font-size: 13px; line-height: 1.5; }
  .stat .lab { width: 132px; flex-shrink: 0; color: #6a7480; }
  .stat .val { flex-shrink: 0; white-space: nowrap; color: #e8ebef; font-weight: 500; }
  .repo {
    display: inline-block; margin-top: 24px; color: #9b7dff; font-size: 13px;
    text-decoration: none; cursor: pointer; transition: color 150ms;
  }
  .repo:hover { color: #7cb8ff; text-decoration: underline; }
  .license { margin-top: 24px; font-size: 12px; color: #5b6472; line-height: 1.4; max-width: 560px; }
</style>
