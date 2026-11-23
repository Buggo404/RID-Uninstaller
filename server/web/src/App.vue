<script setup>
import { computed, onMounted, reactive, ref } from "vue";

async function call(method, path, body) {
  const response = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: body ? { "content-type": "application/json" } : {},
    body: body && JSON.stringify(body),
  });
  const data = await response.json().catch(() => null);
  if (response.status === 401 && !path.endsWith("/login")) me.value = null;
  if (!response.ok) throw new Error(data?.error ?? `HTTP ${response.status}`);
  return data;
}

const me = ref(undefined); // undefined: not known yet
const tab = ref("rules");
const error = ref("");
const done = ref("");
const busy = ref(false);

/** Runs one action: one at a time, its failure shown where the user is looking. */
async function run(work, success = "") {
  if (busy.value) return;
  busy.value = true;
  error.value = done.value = "";
  try {
    await work();
    done.value = success;
  } catch (e) {
    error.value = e.message;
  } finally {
    busy.value = false;
  }
}

// Sign-in

const login = reactive({ username: "", password: "", captcha: "" });
const captcha = ref(null);
const captchaSrc = computed(() => (captcha.value ? `data:image/svg+xml,${encodeURIComponent(captcha.value.svg)}` : ""));

async function newCaptcha() {
  login.captcha = "";
  captcha.value = await call("GET", "/admin/api/captcha").catch(() => null);
}

const signIn = () =>
  run(async () => {
    try {
      me.value = await call("POST", "/admin/api/login", { ...login, captchaId: captcha.value?.id });
      login.password = "";
      await opened();
    } catch (e) {
      // An image is good for one attempt.
      await newCaptcha();
      throw e;
    }
  });

const signOut = () =>
  run(async () => {
    await call("POST", "/admin/api/logout");
    me.value = null;
    await newCaptcha();
  });

// Account

const account = reactive({ currentPassword: "", username: "", password: "", repeat: "" });
const saveAccount = () =>
  run(async () => {
    if (account.password !== account.repeat) throw new Error("两次输入的新密码不一致");
    const forced = me.value.mustChange;
    me.value = await call("POST", "/admin/api/account", account);
    Object.assign(account, { currentPassword: "", password: "", repeat: "" });
    if (forced) await opened();
  }, "已保存");

// Release

const release = reactive({ version: "", url: "", notes: "" });
const publish = () => run(() => call("PUT", "/v1/update", release), "已发布");

// Rules: one per line, "kind path"

const programs = ref([]);
const rule = reactive({ key: "", name: "", text: "", existing: false });

const loadPrograms = async () => (programs.value = await call("GET", "/admin/api/programs"));

const openProgram = (key) =>
  run(async () => {
    const found = await call("GET", `/v1/rules/${encodeURIComponent(key)}`);
    Object.assign(rule, { key, name: found.name, text: found.rules.map((r) => `${r.kind} ${r.path}`).join("\n"), existing: true });
  });

const newProgram = () => Object.assign(rule, { key: "", name: "", text: "", existing: false });

const saveProgram = () =>
  run(async () => {
    const rules = rule.text
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => ({ kind: line.split(/\s+/, 1)[0], path: line.replace(/^\S+\s+/, "") }));
    await call("PUT", `/v1/rules/${encodeURIComponent(rule.key)}`, { name: rule.name, rules });
    rule.existing = true;
    await loadPrograms();
  }, "已保存");

const removeProgram = () =>
  run(async () => {
    await call("DELETE", `/v1/rules/${encodeURIComponent(rule.key)}`);
    newProgram();
    await loadPrograms();
  }, "已删除");

// AI: the model, and one prompt per feature

const features = [["program", "程序"], ["startup", "启动项"], ["leftover", "残留"]];
const feature = ref("program");
const ai = reactive({ protocol: "chat", baseUrl: "", model: "", apiKey: "", hasKey: false, fromEnvironment: false, prompts: {}, defaults: {} });
const tried = ref(null);

const loadAi = async () => Object.assign(ai, await call("GET", "/admin/api/ai"), { apiKey: "" });

const saveAi = () =>
  run(async () => {
    await call("PUT", "/admin/api/ai", { protocol: ai.protocol, baseUrl: ai.baseUrl, model: ai.model, apiKey: ai.apiKey, prompts: ai.prompts });
    await loadAi();
  }, "已保存");

/** Tests what is saved, so the settings on screen are saved first. */
const tryAi = () =>
  run(async () => {
    tried.value = null;
    await call("PUT", "/admin/api/ai", { protocol: ai.protocol, baseUrl: ai.baseUrl, model: ai.model, apiKey: ai.apiKey, prompts: ai.prompts });
    await loadAi();
    tried.value = await call("POST", "/admin/api/ai/test", { feature: feature.value });
  });

/** Loads what the signed-in pages show. */
async function opened() {
  account.username = me.value.username;
  if (me.value.mustChange) return;
  await loadPrograms();
  await loadAi();
  Object.assign(release, await call("GET", "/v1/update").catch(() => ({})));
}

function show(name) {
  tab.value = name;
  error.value = done.value = "";
}

onMounted(async () => {
  me.value = await call("GET", "/admin/api/me").catch(() => null);
  if (me.value) await opened().catch((e) => (error.value = e.message));
  else await newCaptcha();
});
</script>

<template>
  <main v-if="me === null" class="card">
    <h1>RID 管理</h1>
    <form @submit.prevent="signIn">
      <input v-model.trim="login.username" class="input" placeholder="用户名" autocomplete="username" required autofocus />
      <input v-model="login.password" class="input" type="password" placeholder="密码" autocomplete="current-password" required />
      <div class="row">
        <input v-model.trim="login.captcha" class="input" placeholder="验证码" inputmode="numeric" maxlength="5" autocomplete="off" required />
        <button type="button" class="captcha" title="换一张" @click="newCaptcha"><img :src="captchaSrc" alt="验证码" /></button>
      </div>
      <p v-if="error" class="error" role="alert">{{ error }}</p>
      <button class="btn primary" :disabled="busy">登录</button>
    </form>
  </main>

  <main v-else-if="me?.mustChange" class="card">
    <h1>设置账号</h1>
    <p class="note">初始密码只能用来完成这一步。</p>
    <form @submit.prevent="saveAccount">
      <input v-model="account.currentPassword" class="input" type="password" placeholder="初始密码" autocomplete="current-password" required />
      <input v-model.trim="account.username" class="input" placeholder="用户名" autocomplete="username" required />
      <input v-model="account.password" class="input" type="password" placeholder="新密码（至少 10 位）" autocomplete="new-password" minlength="10" required />
      <input v-model="account.repeat" class="input" type="password" placeholder="再输入一次" autocomplete="new-password" required />
      <p v-if="error" class="error" role="alert">{{ error }}</p>
      <button class="btn primary" :disabled="busy">保存</button>
    </form>
  </main>

  <div v-else-if="me" class="shell">
    <header>
      <strong>RID</strong>
      <nav>
        <button v-for="[id, label] in [['rules', '规则库'], ['ai', 'AI'], ['release', '版本'], ['account', '账号']]" :key="id" :class="{ active: tab === id }" @click="show(id)">{{ label }}</button>
      </nav>
      <span class="spacer"></span>
      <span class="note">{{ me.username }}</span>
      <button class="btn ghost" @click="signOut">退出</button>
    </header>

    <section v-if="tab === 'rules'" class="split">
      <ul class="programs">
        <li><button :class="{ active: !rule.existing }" @click="newProgram">＋ 新程序</button></li>
        <li v-for="p in programs" :key="p.key">
          <button :class="{ active: rule.existing && rule.key === p.key }" @click="openProgram(p.key)">
            <span>{{ p.name }}</span><span class="note">{{ p.rules }}</span>
          </button>
        </li>
      </ul>
      <form class="pane" @submit.prevent="saveProgram">
        <div class="row">
          <input v-model.trim="rule.key" class="input" placeholder="键（如 7zip）" :readonly="rule.existing" required />
          <input v-model.trim="rule.name" class="input" placeholder="程序名称" required />
        </div>
        <textarea v-model="rule.text" class="input" spellcheck="false" placeholder="每行一条：dir %APPDATA%\Vendor\App&#10;file / dir / reg_key"></textarea>
        <div class="row">
          <p v-if="error" class="error" role="alert">{{ error }}</p>
          <p v-else class="ok">{{ done }}</p>
          <span class="spacer"></span>
          <button v-if="rule.existing" type="button" class="btn danger" :disabled="busy" @click="removeProgram">删除</button>
          <button class="btn primary" :disabled="busy">保存</button>
        </div>
      </form>
    </section>

    <form v-else-if="tab === 'ai'" class="pane wide" @submit.prevent="saveAi">
      <div class="row">
        <select v-model="ai.protocol" class="input fixed" aria-label="协议">
          <option value="chat">OpenAI chat/completions</option>
          <option value="responses">OpenAI responses</option>
          <option value="messages">Anthropic messages</option>
        </select>
        <input v-model.trim="ai.baseUrl" class="input" placeholder="接口地址，含版本段（如 https://api.example.com/v1）；留空则关闭 AI" spellcheck="false" />
      </div>
      <div class="row">
        <input v-model.trim="ai.model" class="input" placeholder="模型名称" spellcheck="false" />
        <input v-model="ai.apiKey" class="input" type="password" autocomplete="off" :placeholder="ai.hasKey ? '密钥已保存，留空则不修改' : 'API 密钥'" />
      </div>
      <p v-if="ai.fromEnvironment" class="note">当前使用的是 .env 里的配置；在这里保存后以这里为准。</p>

      <nav class="tabs">
        <button v-for="[id, label] in features" :key="id" type="button" :class="{ active: feature === id }" @click="(feature = id), (tried = null)">{{ label }}</button>
      </nav>
      <textarea v-model="ai.prompts[feature]" class="input prompt" spellcheck="false" aria-label="提示词"></textarea>
      <div class="row">
        <p v-if="error" class="error" role="alert">{{ error }}</p>
        <p v-else class="ok">{{ done }}</p>
        <span class="spacer"></span>
        <button type="button" class="btn" :disabled="busy || ai.prompts[feature] === ai.defaults[feature]" @click="ai.prompts[feature] = ai.defaults[feature]">恢复默认</button>
        <button type="button" class="btn" :disabled="busy" @click="tryAi">{{ busy ? "请稍候…" : "保存并测试" }}</button>
        <button class="btn primary" :disabled="busy">保存</button>
      </div>
      <div v-if="tried" class="tried">
        <p><strong>{{ tried.verdict }}</strong> · {{ tried.confidence }}</p>
        <p>{{ tried.summary }}</p>
        <ul><li v-for="p in tried.points" :key="p">{{ p }}</li></ul>
      </div>
    </form>

    <form v-else-if="tab === 'release'" class="pane narrow" @submit.prevent="publish">
      <input v-model.trim="release.version" class="input" placeholder="版本号（如 0.2.0）" pattern="\d+\.\d+\.\d+" required />
      <input v-model.trim="release.url" class="input" type="url" placeholder="下载页地址（https://）" required />
      <textarea v-model="release.notes" class="input" placeholder="更新说明"></textarea>
      <div class="row">
        <p v-if="error" class="error" role="alert">{{ error }}</p>
        <p v-else class="ok">{{ done }}</p>
        <span class="spacer"></span>
        <button class="btn primary" :disabled="busy">发布</button>
      </div>
    </form>

    <form v-else class="pane narrow" @submit.prevent="saveAccount">
      <input v-model.trim="account.username" class="input" placeholder="用户名" autocomplete="username" required />
      <input v-model="account.currentPassword" class="input" type="password" placeholder="当前密码" autocomplete="current-password" required />
      <input v-model="account.password" class="input" type="password" placeholder="新密码（至少 10 位）" autocomplete="new-password" minlength="10" required />
      <input v-model="account.repeat" class="input" type="password" placeholder="再输入一次" autocomplete="new-password" required />
      <div class="row">
        <p v-if="error" class="error" role="alert">{{ error }}</p>
        <p v-else class="ok">{{ done }}</p>
        <span class="spacer"></span>
        <button class="btn primary" :disabled="busy">保存</button>
      </div>
    </form>
  </div>
</template>
