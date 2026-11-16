(() => {
  const qs = new URLSearchParams(location.search);
  let token = qs.get("token") || localStorage.getItem("andon_token") || "";
  if (qs.get("token")) localStorage.setItem("andon_token", token);

  const authHeaders = () => (token ? { Authorization: `Bearer ${token}` } : {});

  async function api(path, opts = {}) {
    const res = await fetch(path, {
      ...opts,
      headers: { ...authHeaders(), "Content-Type": "application/json", ...(opts.headers || {}) },
    });
    if (res.status === 401) {
      token = prompt("ANDON_TOKEN") || "";
      localStorage.setItem("andon_token", token);
      return api(path, opts);
    }
    if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
    return res.status === 204 ? null : res.json();
  }

  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };

  const ago = (ts) => {
    const s = Math.max(0, Math.floor(Date.now() / 1000) - ts);
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.floor(s / 60)}m`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    return `${Math.floor(s / 86400)}d`;
  };

  let mermaid = null;
  async function renderDiagram(container, source) {
    if (!mermaid) {
      mermaid = (await import("https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs")).default;
      mermaid.initialize({ startOnLoad: false, securityLevel: "strict" });
    }
    const id = `m${Math.random().toString(36).slice(2)}`;
    try {
      const { svg } = await mermaid.render(id, source);
      container.innerHTML = svg;
    } catch {
      container.textContent = source;
    }
  }

  function decisionCard(d) {
    const c = el("div", `card ${d.state}`);
    c.append(el("div", "meta", `${d.session_title || d.session_id} · ${ago(d.created_at)}前`));
    c.append(el("div", "title", d.title));
    if (d.summary) c.append(el("div", "body", d.summary));
    else if (d.body) c.append(el("div", "body", d.body));

    if (d.diagram && d.diagram.source) {
      const box = el("div", "diagram");
      c.append(box);
      renderDiagram(box, d.diagram.source);
    }

    if (d.state === "pending") {
      if (d.kind === "approve") {
        const a = el("a", "linkbtn", "Devin で開く");
        a.href = d.session_url;
        a.target = "_blank";
        c.append(a);
      } else {
        const opts = el("div", "opts");
        for (const o of d.options || []) {
          const b = el("button", null, o.label);
          b.onclick = () => submit(d.id, o.id, null, b);
          opts.append(b);
        }
        c.append(opts);
        const free = el("div", "free");
        const input = document.createElement("input");
        input.placeholder = "自由入力で回答…";
        const send = el("button", null, "送信");
        const go = () => input.value.trim() && submit(d.id, null, input.value.trim(), send);
        send.onclick = go;
        input.onkeydown = (e) => e.key === "Enter" && go();
        free.append(input, send);
        c.append(free);
      }
    } else if (d.state === "answered") {
      c.append(el("div", "answered", `回答済み: ${d.answer || ""}`));
    } else {
      c.append(el("div", "expired", "期限切れ(セッションが進行)"));
    }
    return c;
  }

  async function submit(id, optionId, text, btn) {
    btn.disabled = true;
    try {
      await api(`/api/decisions/${id}/answer`, {
        method: "POST",
        body: JSON.stringify({ option_id: optionId, text }),
      });
      await load();
    } catch (e) {
      alert(`送信失敗: ${e.message}`);
      btn.disabled = false;
    }
  }

  function sessionRow(s) {
    // ローカルセッション(local:<id>)は外部リンクを持たないのでリンク化しない
    const link = /^https?:/.test(s.url);
    const a = el(link ? "a" : "div", "srow");
    if (link) {
      a.href = s.url;
      a.target = "_blank";
    }
    const kind = s.status_detail || s.status || "other";
    const dot = el("span", `dot ${["working", "waiting_for_user", "waiting_for_approval", "finished"].includes(kind) ? kind : "other"}`);
    a.append(dot, el("span", "stitle", s.title || s.session_id), el("span", "sst", `${s.status}${s.status_detail ? "/" + s.status_detail : ""} · ${ago(s.updated_at)}`));
    return a;
  }

  async function load() {
    const { sessions, decisions } = await api("/api/state");
    const pending = decisions.filter((d) => d.state === "pending").length;
    document.getElementById("badge").textContent = pending ? `判断待ち ${pending}` : "";

    const dd = document.getElementById("decisions");
    dd.replaceChildren();
    if (!decisions.length) dd.append(el("div", "", "判断待ちはありません"));
    for (const d of decisions) dd.append(decisionCard(d));

    const ss = document.getElementById("sessions");
    ss.replaceChildren();
    if (!sessions.length) ss.append(el("div", "", "セッションがありません"));
    sessions.forEach((s) => ss.append(sessionRow(s)));
  }

  try {
    const es = new EventSource(`/api/events?token=${encodeURIComponent(token)}`);
    es.onmessage = () => load().catch(() => {});
  } catch {}
  load().catch((e) => {
    document.getElementById("decisions").textContent = `読み込み失敗: ${e.message}`;
  });
  setInterval(() => load().catch(() => {}), 30000);
})();
