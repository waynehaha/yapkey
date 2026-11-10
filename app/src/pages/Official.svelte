<script lang="ts">
  import { app } from "../lib/store.svelte";
  import { call, type OfficialStatus } from "../lib/api";

  let key = $state("");
  let busy = $state(false);
  let error = $state("");
  const status = $derived(app.official);

  async function save() {
    busy = true;
    error = "";
    try {
      app.official = await call<OfficialStatus>("official_save_key", { key });
      key = "";
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }

  async function clear() {
    busy = true;
    error = "";
    try {
      app.official = await call<OfficialStatus>("official_clear_key");
      key = "";
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }
</script>

<h3>火山官方语音配置</h3>
<div class="group official">
  <div class="row">
    <div class="lbl">豆包语音 2.0 · 实时识别
      <small>按火山账户的语音识别用量计费。保存密钥不会发起识别</small>
    </div>
    <button class="btn" onclick={() => call("open_url", { url: "https://console.volcengine.com/speech/new/setting/apikeys" })}>火山控制台</button>
  </div>
  <form class="row key-form" onsubmit={(event) => { event.preventDefault(); save(); }}>
    <label class="lbl" for="volc-api-key">API Key
      <small>仅保存在本机钥匙串，不写入项目或普通设置文件</small>
    </label>
    <input id="volc-api-key" class="key" type="password" autocomplete="off" spellcheck="false" placeholder={status.configured ? "粘贴新密钥以替换" : "粘贴火山语音 API Key"} bind:value={key} disabled={busy || status.loading} />
    <button class="btn pri" type="submit" disabled={busy || status.loading || !key.trim()}>{busy ? "处理中…" : status.configured ? "替换密钥" : "保存密钥"}</button>
  </form>
  <div class="row">
    <div class="lbl">
      {#if status.loading}正在读取钥匙串…
      {:else if status.configured}<span class="ok">● 已保存密钥</span>
      {:else}尚未配置密钥{/if}
      <small>启用前需在控制台开通流式语音识别 2.0。此渠道默认不加入多渠道和渠道对比；手动勾选后也会产生用量</small>
    </div>
    {#if status.configured}<button class="btn" disabled={busy} onclick={clear}>移除密钥</button>{/if}
  </div>
  {#if error || status.error}<div class="row"><small class="err" role="status">{error || status.error}</small></div>{/if}
</div>

<style>
  .key-form { flex-wrap: wrap; }
  .key-form .lbl { flex: 1 1 200px; }
  .key { flex: 1 1 210px; min-width: 140px; max-width: 340px; border: 1px solid var(--line); background: var(--card2); border-radius: var(--rs); padding: 7px 10px; outline: none; -webkit-user-select: text; user-select: text; }
  .key:focus { border-color: var(--accent); }
  .key::placeholder { color: var(--fg3); }
  .lbl small { line-height: 1.6; }
  .err { color: var(--err); overflow-wrap: anywhere; }
</style>
