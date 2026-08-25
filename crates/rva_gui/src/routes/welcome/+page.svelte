<script lang="ts">
  // Task A: standalone welcome page (initial route). Focuses on conveying info and no longer serves as the file-open entry;
  // opening files is handled by the main-view toolbar + Ctrl+O. After checking "Don't show again", future launches go straight to the main view.
  import { goto } from "$app/navigation";

  declare const __APP_VERSION__: string;

  // The welcome page shows only the bare version, stripping the build timestamp (timestamp is for the HUD to tell new/old builds apart).
  const APP_VER = __APP_VERSION__.split("-b")[0];

  let skip = $state(false);
  let showLog = $state(false);

  const SHORTCUTS: { key: string; desc: string }[] = [
    { key: "Ctrl+O", desc: "打开" },
    { key: "Prev", desc: "上一个差异" },
    { key: "Next", desc: "下一个差异" },
    { key: "Go", desc: "跳转偏移" },
    { key: "Ctrl+Z", desc: "撤销" },
  ];

  const STEPS: { n: string; title: string; desc: string }[] = [
    { n: "1", title: "导入文件", desc: "拖拽两份二进制文件，或使用 Ctrl+O 打开" },
    { n: "2", title: "选择策略比对", desc: "分块哈希 / 滑动窗口 / 函数级匹配三种引擎" },
    { n: "3", title: "查看 / 编辑 / 导出", desc: "差异高亮、逐字节编辑、撤销与补丁导出" },
  ];

  interface LogItem { t: "+" | "•" | "~"; s: string }
  interface LogVer { version: string; items: LogItem[] }
  const LOGS: LogVer[] = [
    {
      version: "0.2.1",
      items: [
        { t: "+", s: "搜索支持 A / B / 两侧同时搜索，命中按偏移合并，字节级精确高亮（淡黄=普通命中、亮黄=当前命中）" },
        { t: "•", s: "统一复制序列化链路：工具栏复制、右键控制板、HexPanel 复制（Ctrl+C）共用全局复制格式设置" },
        { t: "•", s: "编辑单字节后自动小范围重新比对并保留滚动位置（去抖 250ms）" },
        { t: "•", s: "修复右键命中误判为左键选中、选中另一字节误呼出控制板" },
        { t: "~", s: "差异 HUD 默认关闭（设置页可重新开启）" },
      ],
    },
    {
      version: "0.2.0",
      items: [
        { t: "+", s: "搜索支持 HEX 序列（FF 15 90 / 0xFF15）与 ASCII/UTF-8 文本自动识别，纯字母单词不再被误判成字节" },
        { t: "+", s: "行号区支持按住拖选多行；A / B 面板拖选实时联动，选中状态全局唯一" },
        { t: "+", s: "全局拦截浏览器默认右键菜单，统一使用应用内控制面板" },
        { t: "•", s: "修复 hex 选中底板不铺满：选中框覆盖整个字节槽，连续选中无缝衔接" },
        { t: "•", s: "修复跳转「下一处 / 上一处」后对侧面板旧高亮不清除" },
        { t: "•", s: "修复编辑单个字节后差异不自动重算（提交编辑即重新比对并回到编辑位置）" },
        { t: "•", s: "修复滚动到底部空白过多：末行贴底，滚动范围与文件实际内容一致" },
        { t: "•", s: "修复双击行号命中 y 偏移误判（亚像素滚动后行定位准确）" },
        { t: "~", s: "撤销 / 重做后自动重算差异，不再需要手动点击「重新比对」" },
      ],
    },
    {
      version: "0.1.6",
      items: [
        { t: "•", s: "补丁页选好 A / B 文件后切回比对页，自动开始比对（无需再手动点一次「比对」按钮）" },
      ],
    },
    {
      version: "0.1.5",
      items: [
        { t: "•", s: "设置页密钥列表每行新增「复制指纹」按钮，点击后顶部滑入提示框；复制公钥同样有滑入反馈" },
        { t: "•", s: "补丁页可直接选择 A / B 文件（输入框 + 浏览按钮），与比对页双向联动，不再必须切到比对页选择" },
        { t: "•", s: "补丁页内容不再被裁剪，长页面可滚动到底" },
        { t: "•", s: "补丁页重进保留进度：路径、签名策略等输入自动保存，切换页面不丢失" },
        { t: "•", s: "补丁文件被篡改时给出明确提示「文件可能已被篡改或损坏」，不再显示晦涩的「不支持的容器版本」" },
        { t: "~", s: "连续点击复制按钮时，每次操作都会重新滑出提示框（不再只有第一次有反馈）" },
      ],
    },
    {
      version: "0.1.4",
      items: [
        { t: "•", s: "修复滚动「0..N 行循环」：滚动条改为全量覆盖整个文件，拖动连续不弹回，窗口随滚动位置自动切换" },
        { t: "•", s: "新增/移除识别修复：插入/删除导致后续内容位移时不再误判成一串就地修改，新增显示绿色、移除显示红色，并可正常跳转" },
        { t: "•", s: "修复「下一处/上一处」只有修改能跳、新增和移除跳不动（新增/移除条目标记了正确落点）" },
        { t: "•", s: "切页返回恢复到真实选中位置（选中条目变化时同步保存，不再回到第一个差异点）" },
        { t: "~", s: "设置页字体大小滑块的紫色填充条改为「左侧 → 圆圈」实时跟随数值" },
      ],
    },
    {
      version: "0.1.3",
      items: [
        { t: "•", s: "滚动不再覆盖当前选中：左下角进度以选中的行为最高优先级，跳转更稳定" },
        { t: "•", s: "点击差异行改为「按行」命中（同行多差异取序号最小），并修复双击选中后出现双选中行" },
        { t: "•", s: "切换视图返回后恢复选中位置，跳转落点有两次闪烁提示（与选中行同色）" },
        { t: "~", s: "差异行背景加强对比：新增绿 / 修改黄 / 删除红更易区分" },
      ],
    },
    {
      version: "0.1.2",
      items: [
        { t: "•", s: "修复切换视图（设置 / 符号）后返回时比对结果与进度丢失" },
        { t: "•", s: "修复新增 / 移除条目点击跳转失效、选中行后跳转停在原地（按条目类型取定位偏移）" },
        { t: "•", s: "修复滚到窗口底部被拉回：切窗后两面板统一从新窗口顶继续（重叠 100 行视觉连续）" },
        { t: "•", s: "点击非差异字符 / 行不再把左下角计数清零，保留当前进度" },
        { t: "•", s: "复制十六进制区选区同时输出 ASCII 列（所见即所得，与导出格式一致）" },
        { t: "~", s: "键盘 Tab 聚焦主按钮与侧边导航有明确高亮反馈" },
      ],
    },
    {
      version: "0.1.1",
      items: [
        { t: "+", s: "性能保护：只加载 / 滚动 1000 行窗口，滚到边界自动切换窗口，配合行号跳转进入大文件任意位置" },
        { t: "+", s: "行号跳转功能（位于顶部工具行，替代原设置按钮位置）" },
        { t: "•", s: "修复下拉框新增 / 移除条目点击不跳转（滚动跟随改为区间覆盖判定）" },
        { t: "•", s: "修复「上一处 / 下一处」计数与可视区概率性不同步" },
        { t: "+", s: "ASCII 区拖选复制纯文本（所见即所得，不可打印字节映射为 .）" },
        { t: "~", s: "欢迎页序号改纯文本、关于面板排版修复、主按钮紫色降强度" },
      ],
    },
    {
      version: "0.1.0",
      items: [
        { t: "+", s: "设置并入主视图侧边导航，显示 / 比对 / 调试配置即时生效" },
        { t: "+", s: "双击行号选中整行，对侧面板同步高亮" },
        { t: "+", s: "差异条目支持展开查看三类型字节对比" },
        { t: "~", s: "sliding 窗口大小可配置（4–64）" },
      ],
    },
    {
      version: "0.0.9",
      items: [
        { t: "+", s: "补丁往返验证 4/4 PASS，补丁导出可用" },
        { t: "•", s: "修复双面板滚动亚像素错位（像素级同步）" },
      ],
    },
    {
      version: "0.0.8",
      items: [
        { t: "+", s: "函数级结构匹配（structural 策略）" },
        { t: "~", s: "大文件分块缓存，滚动不再重复读盘" },
      ],
    },
    {
      version: "0.0.7",
      items: [
        { t: "+", s: "首个可用版本：双栏十六进制视图 + 差异高亮" },
        { t: "•", s: "修复删除检测漏报（F1 0 → 1.0）" },
      ],
    },
  ];

  const LOG_CLASS: Record<LogItem["t"], string> = { "+": "add", "•": "fix", "~": "opt" };

  function start() {
    try {
      localStorage.setItem("rva_skip_welcome", skip ? "1" : "0");
    } catch {
      /* ignore */
    }
    goto("/");
  }
</script>

<svelte:head><title>RVA Compare</title></svelte:head>

<div class="page">
  <!-- Top brand area -->
  <header class="brand">
    <h1 class="brand-title">RVA Compare</h1>
    <p class="brand-sub">轻量二进制差异对比工具 —— 面向安全分析与固件逆向</p>
    <span class="brand-ver">v{APP_VER}</span>
  </header>

  <!-- Main two-column area -->
  <div class="cols">
    <!-- Left column: usage instructions -->
    <section class="card left-card">
      <h2 class="card-title">使用说明</h2>
      <div class="steps">
        {#each STEPS as s}
          <div class="step">
            <span class="step-n">{s.n}.</span>
            <div class="step-body">
              <div class="step-title">{s.title}</div>
              <div class="step-desc">{s.desc}</div>
            </div>
          </div>
        {/each}
      </div>
      <div class="shortcut-box">
        <div class="shortcut-grid">
          {#each SHORTCUTS as s}
            <div class="shortcut-item">
              <span class="kbd">{s.key}</span>
              <span class="sc-desc">{s.desc}</span>
            </div>
          {/each}
        </div>
      </div>
    </section>

    <!-- Right column: changelog / announcements -->
    <section class="card right-card">
      <div class="changelog-head">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" /></svg>
        <span class="cl-title">更新公告</span>
        <span class="cl-ver">v{APP_VER}</span>
      </div>
      <div class="cl-divider"></div>
      <div class="timeline">
        {#each LOGS as log, i}
          <div class="ver-block" class:latest={i === 0}>
            <div class="tl-node" class:latest={i === 0}></div>
            <div class="ver-head">
              <span class="ver-name">{log.version}</span>
            </div>
            <ul class="log-items">
              {#each log.items as item}
                <li class="log-item">
                  <span class="sym {LOG_CLASS[item.t]}">{item.t}</span>
                  <span class="log-text">{item.s}</span>
                </li>
              {/each}
            </ul>
          </div>
        {/each}
      </div>
      <button class="cl-more" onclick={() => (showLog = true)}>查看完整更新日志</button>
    </section>
  </div>

  <!-- Bottom enter button area -->
  <footer class="enter">
    <button class="start-btn" onclick={start}>
      开始使用
    </button>
    <label class="skip-check">
      <input type="checkbox" bind:checked={skip} />
      <span class="box" aria-hidden="true">
        {#if skip}
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5" /></svg>
        {/if}
      </span>
      <span class="skip-label">不再显示</span>
    </label>
  </footer>

  <!-- Full changelog modal -->
  {#if showLog}
    <div class="modal-mask" onclick={(e) => { if (e.target === e.currentTarget) showLog = false; }}>
      <div class="modal">
        <div class="modal-head">
          <span class="modal-title">完整更新日志</span>
          <button class="modal-close" onclick={() => (showLog = false)} aria-label="关闭">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6L6 18M6 6l12 12" /></svg>
          </button>
        </div>
        <div class="modal-body">
          {#each LOGS as log}
            <div class="modal-ver">
              <div class="ver-head">
                <span class="ver-name">{log.version}</span>
              </div>
              <ul class="log-items">
                {#each log.items as item}
                  <li class="log-item">
                    <span class="sym {LOG_CLASS[item.t]}">{item.t}</span>
                    <span class="log-text">{item.s}</span>
                  </li>
                {/each}
              </ul>
            </div>
          {/each}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  * { box-sizing: border-box; }
  html, body { margin: 0; padding: 0; }
  .page {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    min-height: 100vh;
    overflow: hidden;
    background:
      radial-gradient(1200px 600px at 50% -10%, rgba(124, 108, 240, 0.05), transparent 60%),
      radial-gradient(900px 500px at 15% 110%, rgba(124, 108, 240, 0.03), transparent 55%),
      #000000;
    color: #e8eaf0;
    font-family: "Segoe UI", "Microsoft YaHei", system-ui, sans-serif;
    padding-bottom: 48px;
  }

  /* — Top brand area — */
  .brand {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-end;
    height: 100px;
    padding: 32px 0 16px;
    width: 100%;
    animation: rise 200ms cubic-bezier(0.4, 0, 0.2, 1) both;
  }
  @keyframes rise {
    from { opacity: 0; transform: translateY(10px); }
    to { opacity: 1; transform: translateY(0); }
  }
  .brand-title {
    margin: 0;
    font-size: 34px;
    font-weight: 700;
    letter-spacing: 1px;
    color: #f0eef8;
  }
  .brand-title::first-letter { color: #a78bfa; }
  .brand-sub {
    margin: 10px 0 0;
    font-size: 13px;
    font-weight: 400;
    color: #8b8f9c;
    line-height: 1.4;
    letter-spacing: 0.3px;
  }
  .brand-ver {
    position: absolute;
    right: 32px;
    top: 50%;
    transform: translateY(-50%);
    font-size: 11px;
    font-family: Consolas, "Courier New", monospace;
    color: #a78bfa;
    background: rgba(167, 139, 250, 0.1);
    border: 1px solid rgba(167, 139, 250, 0.2);
    border-radius: 20px;
    padding: 3px 10px;
    letter-spacing: 0.5px;
  }

  /* — Main two-column area — */
  .cols {
    position: relative;
    display: flex;
    gap: 24px;
    width: 960px;
    max-width: calc(100vw - 64px);
    height: 520px;
    margin-top: 40px;
  }
  .card {
    width: 468px;
    flex-shrink: 0;
    border-radius: 12px;
    background: #0c0c12;
    border: 1px solid #1c1c26;
    padding: 24px;
    display: flex;
    flex-direction: column;
  }
  .left-card { animation: rise-l 200ms cubic-bezier(0.4, 0, 0.2, 1) 100ms both; }
  .right-card { animation: rise-l 200ms cubic-bezier(0.4, 0, 0.2, 1) 180ms both; }
  @keyframes rise-l {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }
  .card-title {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    color: #e8eaf0;
  }

  /* Left column: usage guide */
  .steps { margin-top: 20px; display: flex; flex-direction: column; gap: 16px; }
  .step { display: flex; gap: 12px; align-items: flex-start; }
  .step-n {
    flex-shrink: 0;
    color: #a78bfa;
    font-family: Consolas, "Courier New", monospace;
    font-size: 12px; font-weight: 700; line-height: 1.6; letter-spacing: 0.5px;
    padding-top: 1px;
  }
  .step-title { font-size: 13px; font-weight: 600; color: #e8eaf0; }
  .step-desc { margin-top: 3px; font-size: 12px; color: #8b8f9c; line-height: 1.5; }

  /* Left column bottom: keyboard shortcut quick reference */
  .shortcut-box {
    margin-top: auto;
    background: #08080c;
    border-radius: 8px;
    padding: 12px;
  }
  .shortcut-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    column-gap: 12px;
    row-gap: 8px;
  }
  .shortcut-item { display: flex; align-items: center; gap: 8px; }
  .kbd {
    background: #1c1c26; border-radius: 4px; padding: 2px 6px;
    font-family: Consolas, "Courier New", monospace;
    font-size: 11px; font-weight: 500; color: #b8bcc8;
  }
  .sc-desc { font-size: 11px; color: #6b6f7c; }

  /* Right column: changelog */
  .changelog-head {
    display: flex; align-items: center; gap: 8px; height: 40px;
    color: #a78bfa; flex-shrink: 0;
  }
  .cl-title { font-size: 16px; font-weight: 600; color: #e8eaf0; }
  .cl-ver { margin-left: auto; font-size: 12px; color: #8b8f9c; }
  .cl-divider { height: 1px; background: #1c1c26; margin-top: 12px; flex-shrink: 0; }

  .timeline {
    position: relative;
    margin-top: 16px;
    overflow-y: auto;
    flex: 1;
    padding: 4px 4px 4px 0;
    scrollbar-width: thin;
    scrollbar-color: #1c1c26 #0c0c12;
  }
  .timeline::-webkit-scrollbar { width: 6px; }
  .timeline::-webkit-scrollbar-track { background: #0c0c12; }
  .timeline::-webkit-scrollbar-thumb { background: #1c1c26; border-radius: 3px; transition: background 150ms; }
  .timeline::-webkit-scrollbar-thumb:hover { background: #2a2a38; }

  .ver-block {
    position: relative;
    padding: 0 0 16px 24px;
  }
  .ver-block:not(:last-child)::before {
    content: "";
    position: absolute;
    left: 3.5px;
    top: 14px;
    bottom: 0;
    width: 1px;
    background: #262033;
  }
  .ver-block.latest {
    background: rgba(167, 139, 250, 0.04);
    border: 1px solid rgba(167, 139, 250, 0.16);
    border-radius: 8px;
    padding: 12px 12px 12px 36px;
    margin-left: -12px;
    margin-right: 4px;
  }
  .tl-node {
    position: absolute;
    left: 0;
    top: 5px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #a78bfa;
    border: 2px solid rgba(167, 139, 250, 0.3);
    box-sizing: content-box;
  }
  .ver-block.latest .tl-node { left: 12px; }
  .ver-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 4px;
  }
  .ver-name { font-size: 13px; font-weight: 600; color: #e8eaf0; }
  .log-items { margin: 6px 0 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 6px; }
  .log-item { display: flex; gap: 8px; font-size: 12px; color: #b8bcc8; line-height: 1.5; }
  .sym { flex-shrink: 0; font-weight: 600; width: 10px; text-align: center; }
  .sym.add { color: #a78bfa; }
  .sym.fix { color: #f2c14e; }
  .sym.opt { color: #7c9aff; }

  .cl-more {
    margin-top: 16px;
    align-self: flex-end;
    background: none; border: none; cursor: pointer;
    font-size: 12px; color: #a78bfa; padding: 0;
    transition: color 150ms;
  }
  .cl-more:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
    text-decoration-thickness: 1px;
    transition: text-decoration-color 100ms;
    color: #c4b5fd;
  }

  /* — Bottom enter button area — */
  .enter {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 32px;
    animation: rise-b 200ms cubic-bezier(0.4, 0, 0.2, 1) 300ms both;
  }
  @keyframes rise-b {
    from { opacity: 0; transform: translateY(8px); }
    to { opacity: 1; transform: translateY(0); }
  }
  .start-btn {
    position: relative;
    overflow: hidden;
    width: 160px;
    height: 44px;
    border-radius: 8px;
    border: none;
    background: #7c6cf0;
    color: #ffffff;
    font-size: 15px;
    font-weight: 600;
    cursor: pointer;
    transition: background 150ms, box-shadow 200ms;
  }
  .start-btn:hover {
    background: #8b7cf7;
    box-shadow: 0 4px 18px rgba(124, 108, 240, 0.35);
  }
  .start-btn:active { background: #6a5adb; transform: translateY(1px); }

  .skip-check {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    user-select: none;
  }
  .skip-check input { position: absolute; opacity: 0; pointer-events: none; }
  .box {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    border: 1px solid #1c1c26;
    background: transparent;
    color: #ffffff;
    transition: background 150ms, border-color 150ms;
  }
  .skip-check input:checked + .box {
    background: #7c6cf0;
    border-color: #7c6cf0;
  }
  .skip-check input:focus-visible + .box {
    outline: 2px solid #7c6cf0;
    outline-offset: 2px;
  }
  .skip-label { font-size: 12px; color: #8b8f9c; }

  /* — Full changelog modal — */
  .modal-mask {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    animation: fadein 150ms ease-out;
  }
  @keyframes fadein { from { opacity: 0; } to { opacity: 1; } }
  .modal {
    width: 560px;
    max-width: calc(100vw - 64px);
    max-height: 70vh;
    background: #0c0c12;
    border: 1px solid #1c1c26;
    border-radius: 12px;
    display: flex;
    flex-direction: column;
    animation: pop 200ms cubic-bezier(0.4, 0, 0.2, 1);
  }
  @keyframes pop {
    from { opacity: 0; transform: scale(0.97) translateY(8px); }
    to { opacity: 1; transform: scale(1) translateY(0); }
  }
  .modal-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 52px;
    padding: 0 20px;
    border-bottom: 1px solid #1c1c26;
    flex-shrink: 0;
  }
  .modal-title { font-size: 15px; font-weight: 600; color: #e8eaf0; }
  .modal-close {
    width: 28px; height: 28px; border-radius: 6px;
    background: transparent; border: none; color: #8b8f9c; cursor: pointer;
    display: flex; align-items: center; justify-content: center;
    transition: background 150ms, color 150ms;
  }
  .modal-close:hover { background: #1f2230; color: #e8eaf0; }
  .modal-body {
    padding: 16px 20px 20px;
    overflow-y: auto;
    scrollbar-width: thin;
    scrollbar-color: #1c1c26 #0c0c12;
  }
  .modal-body::-webkit-scrollbar { width: 6px; }
  .modal-body::-webkit-scrollbar-thumb { background: #1c1c26; border-radius: 3px; }
  .modal-ver { padding-bottom: 16px; }
  .modal-ver:last-child { padding-bottom: 0; }
</style>
