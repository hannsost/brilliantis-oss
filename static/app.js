"use strict";
const $ = (s, root = document) => root.querySelector(s);
const esc = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const money = (n) =>
  new Intl.NumberFormat("de-DE", {
    style: "currency",
    currency: "EUR",
    maximumFractionDigits: 0,
  }).format(n / 100);
const date = (s) =>
  new Intl.DateTimeFormat("de-DE", {
    day: "2-digit",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(s));
const paths = {
  overview: "M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z",
  pipeline: "M4 4h4v16H4z M10 4h4v11h-4z M16 4h4v7h-4z",
  campaigns: "M3 10v4l13 5V5L3 10z M7 16l2 5h3l-2-4 M20 9v6",
  products: "m12 3 9 5-9 5-9-5 9-5z M3 8v9l9 5 9-5V8 M12 13v9",
  agents: "m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5L12 3z",
  integrations:
    "M8 3v5H3 M16 21v-5h5 M3 8a9 9 0 0 1 16-3 M21 16a9 9 0 0 1-16 3",
  roadmap: "M4 3v18 M4 5h14l-3 4 3 4H4",
  plus: "M12 5v14 M5 12h14",
  arrow: "M5 12h14 M14 7l5 5-5 5",
  chart: "M4 19V5 M4 19h17 M8 14l4-4 4 2 5-6",
  clock: "M12 8v5l3 2 M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0",
  check: "m5 12 4 4L19 6",
  close: "m6 6 12 12 M6 18 18 6",
  import: "M12 3v12 M7 10l5 5 5-5 M4 16v5h16v-5",
};
const icon = (name) =>
  `<svg class="icon" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${paths[name] || paths.products}"/></svg>`;
const nav = [
  ["overview", "Übersicht"],
  ["pipeline", "Sales Pipeline"],
  ["campaigns", "Kampagnen"],
  ["influencers", "Influencer"],
  ["products", "Produkte & Angebote"],
  ["agents", "Agenten & Aktivitäten"],
  ["integrations", "Integrationen"],
  ["roadmap", "ERP Ausbau"],
  ["distribution", "Produkt & Service"],
];
const stages = {
  new: "Neu",
  qualified: "Qualifiziert",
  proposal: "Angebot",
  won: "Gewonnen",
  lost: "Verloren",
};
const campaignStates = {
  draft: "Entwurf",
  planned: "Geplant",
  paused: "Pausiert",
};
const channels = {
  email: "E-Mail",
  content: "Content",
  partner: "Partner",
  event: "Event",
  seo: "SEO",
  social: "Social Media",
  forum: "Forum",
  influencer: "Influencer",
};
const sources = {
  manual: "Manuell",
  website: "Website",
  referral: "Empfehlung",
  campaign: "Kampagne",
  partner: "Partner",
};
const state = {
  data: null,
  events: [],
  view: "overview",
  query: "",
  filter: "all",
};
let toastTimer;
const pendingCommands = new Map();
async function api(path, options = {}) {
  const headers = new Headers(options.headers || {});
  if (window.BrilliantisAccess.csrf && options.method && options.method !== "GET") headers.set("X-CSRF-Token", window.BrilliantisAccess.csrf);
  const response = await fetch(path, { ...options, headers, credentials: "same-origin" });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    if (response.status === 401) { window.BrilliantisAccess.user = null; location.reload(); }
    throw new Error(data.error?.message || `Anfrage fehlgeschlagen (${response.status})`);
  }
  return data;
}
async function command(path, method, data) {
  const body = JSON.stringify(data),
    signature = method + path + body;
  if (!pendingCommands.has(signature))
    pendingCommands.set(signature, crypto.randomUUID());
  const result = await api(path, {
    method,
    body,
    headers: {
      "Content-Type": "application/json",
      "Idempotency-Key": pendingCommands.get(signature),
      "X-Brilliantis-Client": "local.gui",
    },
  });
  pendingCommands.delete(signature);
  return result;
}
async function load() {
  const [data, events] = await Promise.all([
    api("/api/v1/dashboard"),
    allEvents(),
  ]);
  state.data = data;
  const profile = data.distribution;
  $("#distribution-label").textContent = profile.id === "neurofunken" ? "für NeuroFunken" : "Standalone · MIT";
  $("#workspace-name").textContent = profile.workspace;
  $("#profile-name").textContent = profile.workspace;
  $("#workspace-tagline").textContent = profile.tagline;
  $("#footer-tagline").textContent = profile.label;
  document.querySelectorAll("[data-initials]").forEach((element) => { element.textContent = profile.initials; });
  state.events = events.slice(-12).reverse();
  $("#workspace-mode").textContent =
    data.mode === "demo" ? "Demo · lokale Daten" : "Lokaler Arbeitsbereich";
}
async function allEvents() { return (await api('/api/v1/events?latest=true')).items; }

function toast(message) {
  clearTimeout(toastTimer);
  $("#toast").textContent = message;
  $("#toast").classList.add("visible");
  toastTimer = setTimeout(() => $("#toast").classList.remove("visible"), 4200);
}
function productName(id) {
  return state.data.catalog.find((p) => p.id === id)?.name || id;
}
function badge(status, campaign = false) {
  return `<span class="badge ${esc(status)}">${esc((campaign ? campaignStates : stages)[status] || status)}</span>`;
}
function btn(label, action, primary = false, ic = "plus") {
  return `<button class="button ${primary ? "primary" : ""}" data-action="${action}">${icon(ic)}${label}</button>`;
}
function heading(eyebrow, title, description, actions = "") {
  return `<div class="page-heading"><div><div class="eyebrow">${eyebrow}</div><h1>${title}</h1><p>${description}</p></div><div class="heading-actions">${actions}</div></div>`;
}
function demoNote() {
  return state.data.mode === "demo"
    ? '<div class="demo-note"><span>◈</span><span><strong>Dein lokaler Spielraum.</strong> Alle Firmen, Leads und Beträge sind erfundene Beispieldaten. Änderungen bleiben gespeichert.</span></div>'
    : "";
}
function panelHeader(title, subtitle = "", link = "") {
  return `<div class="panel-head"><div><h2>${title}</h2>${subtitle ? `<p class="panel-subtitle">${subtitle}</p>` : ""}</div>${link}</div>`;
}
function metric(title, value, foot, ic) {
  return `<div class="metric"><div class="metric-top">${title}<span class="metric-icon">${icon(ic)}</span></div><div class="metric-value">${value}</div><div class="metric-foot">${foot}</div></div>`;
}
function overview() {
  const d = state.data,
    m = d.metrics;
  const groups = ["new", "qualified", "proposal", "won"].map((status) => ({
    status,
    items: d.leads.filter((l) => l.status === status),
    amount: d.pipeline?.find(g=>g.status===status)?.amount_cents || 0,
    count: d.pipeline?.find(g=>g.status===status)?.count || 0,
  }));
  const max = Math.max(
    1,
    ...groups.map((g) => g.amount),
  );
  const first = d.leads.find((l) => !["won", "lost"].includes(l.status));
  const recent = d.leads.slice(0, 5);
  return (
    heading(
      "DEIN GESCHÄFT IM BLICK",
      "Gute Ideen. Nächster Schritt.",
      "Vertrieb, Produkte und Agenten an einem Ort.",
      btn("Kampagne planen", "new-campaign", false, "campaigns") +
        btn("Neuer Lead", "new-lead", true),
    ) +
    demoNote() +
    `<div class="metric-grid">${metric("Offene Pipeline", money(m.pipeline_cents), `${m.open_leads} offene Verkaufschancen`, "pipeline")}${metric("Gewichteter Forecast", money(m.weighted_cents), "10 / 35 / 70 % nach Vertriebsphase", "chart")}${metric("Gewonnene Deals", money(m.won_cents), "Dealvolumen · kein gebuchter Umsatz", "check")}${metric("Kampagnen", m.campaigns, "Entwürfe & Planung · ohne Versand", "campaigns")}</div>
    <div class="main-grid"><section class="panel">${panelHeader("Von Interesse zu Zusammenarbeit", "Dealvolumen nach Vertriebsphase", '<span class="period">Aktueller Bestand</span>')}<div class="panel-body"><div class="pipeline-chart">${groups
      .map((g) => {
        const n = g.amount;
        return `<div class="bar-group"><strong>${money(n)}</strong><svg width="90" height="${Math.max(7, Math.round((n / max) * 130))}" class="chart-bar" role="img" aria-label="${stages[g.status]}: ${esc(money(n))}"><title>${esc(money(n))}</title></svg></div>`;
      })
      .join(
        "",
      )}</div><div class="chart-labels">${groups.map((g) => `<div>${stages[g.status]} · ${g.count}</div>`).join("")}</div><p class="chart-foot"><i></i> EUR netto · einmaliger geschätzter Auftragswert</p></div></section>
    <section class="panel agent-panel">${panelHeader(`<span class="agent-emblem">${icon("agents")}</span>`, "", '<span class="panel-tag">AGENTENBEREIT</span>')}<div class="panel-body"><div class="agent-label">MENSCH & AGENT</div><h2>Der nächste gute Schritt.</h2><p class="intro">Klare Aufgaben, gemeinsamer Kontext. Deine Agenten arbeiten über dieselbe API wie du.</p><div class="agent-task">${icon("arrow")}<div><strong>${first ? esc(first.company) : "Platz für deinen ersten Lead"}</strong><small>${first ? esc(first.next_action) : "Lege einen Lead an und hinterlege die nächste Aktion."}</small></div></div><button class="text-button" data-nav="agents">Agentenkontext ansehen ↗</button></div></section></div>
    <div class="bottom-grid"><section class="panel">${panelHeader("Deine Verkaufschancen", "Was als Nächstes wichtig wird", '<button class="text-button" data-nav="pipeline">Alle ansehen ↗</button>')}${leadTable(recent)}</section><section class="panel">${panelHeader("Neu in Brilliantis", "Zuletzt lokal gespeichert")}<div class="panel-body">${
      state.events
        .slice(0, 4)
        .map(
          (e) =>
            `<div class="activity"><span class="activity-dot"></span><div><p>${esc(e.summary)}</p><small>${esc(e.actor_label)} · ${esc(date(e.created_at))}</small></div></div>`,
        )
        .join("") ||
      '<div class="empty">Hier erscheinen deine ersten Aktivitäten.</div>'
    }</div></section></div>
    <div class="ecosystem-strip"><strong>${esc(d.distribution.label)}</strong><div class="system-names">${d.distribution.showcase.map((name) => `<span>${esc(name)}</span>`).join("")}</div><small>Anbindungen geplant</small></div>`
  );
}
function leadTable(leads) {
  return leads.length
    ? `<div class="table-scroll"><table><thead><tr><th>Unternehmen / Produkt</th><th>Phase</th><th>Volumen</th><th><span class="muted">Details</span></th></tr></thead><tbody>${leads.map((l) => `<tr><td><div class="company-cell"><span class="company-logo">${esc(l.company.slice(0, 1))}</span><div><strong>${esc(l.company)}</strong><small>${esc(productName(l.product_id))}</small></div></div></td><td>${badge(l.status)}</td><td class="no-wrap">${money(l.amount_cents)}</td><td><button class="icon-button" data-lead="${esc(l.id)}" aria-label="${esc(l.company)} öffnen">↗</button></td></tr>`).join("")}</tbody></table></div>`
    : '<div class="empty">Alles beginnt mit einem Gespräch.<br>Lege deinen ersten Lead an.</div>';
}
function pipeline() {
  const leads = state.data.leads.filter(
    (l) =>
      (state.filter === "all" || l.product_id === state.filter) &&
      [l.company, l.title, l.contact, l.next_action]
        .join(" ")
        .toLocaleLowerCase()
        .includes(state.query.toLocaleLowerCase()),
  );
  return (
    heading(
      "VON LEAD ZU KUNDE",
      "Deine Sales Pipeline.",
      "Jede Chance mit Kontext, Volumen und einem konkreten nächsten Schritt.",
      btn("Website-Import", "import", false, "import") +
        btn("Neuer Lead", "new-lead", true),
    ) +
    demoNote() +
    `<div class="toolbar"><input class="search" id="lead-search" type="search" aria-label="Leads durchsuchen" placeholder="Unternehmen, Kontakt oder nächste Aktion suchen …" value="${esc(state.query)}"><select class="filter" id="product-filter" aria-label="Nach Produkt filtern"><option value="all">Alle Produkte</option>${state.data.catalog.map((p) => `<option value="${p.id}" ${state.filter === p.id ? "selected" : ""}>${esc(p.name)}</option>`).join("")}</select></div><div class="board">${Object.entries(
      stages,
    )
      .map(([status, label]) => {
        const items = leads.filter((l) => l.status === status);
        return `<section class="board-column"><div class="column-heading">${label}<span class="badge">${items.length}</span></div><div class="column-value">${money(items.reduce((s, l) => s + l.amount_cents, 0))}</div>${items.map((l) => `<button class="lead-card" data-lead="${esc(l.id)}"><span class="product-tag">${esc(productName(l.product_id))}</span><h3>${esc(l.company)}</h3><p>${esc(l.title)}</p><strong>${money(l.amount_cents)}</strong><small>↗ ${esc(l.next_action)}</small></button>`).join("") || '<div class="empty">Noch keine Leads</div>'}</section>`;
      })
      .join("")}</div>`
  );
}
function campaigns() {
  return (
    heading(
      "AUS IDEEN WERDEN GESPRÄCHE",
      "Kampagnen mit Richtung.",
      "Zielgruppe, Botschaft und Budget – gemeinsam mit deinem Sales-Team planen.",
      btn("Neue Kampagne", "new-campaign", true),
    ) +
    demoNote() +
    `<p class="section-intro">Hier entstehen Kampagnenentwürfe. „Geplant“ speichert den Plan; ein Versanddienst ist noch nicht verbunden.</p><div class="card-grid">${state.data.campaigns.map((c) => `<article class="panel campaign-card"><div>${badge(c.status, true)} <span class="badge">${esc(channels[c.channel])}</span></div><h2>${esc(c.name)}</h2><p>${esc(c.objective)}</p><dl><div><dt>Produkt</dt><dd>${esc(productName(c.product_id))}</dd></div><div><dt>Budgetrahmen</dt><dd>${money(c.budget_cents)}</dd></div><div><dt>Zielgruppe</dt><dd>${esc(c.audience)}</dd></div></dl><button class="button" data-campaign="${esc(c.id)}">Plan ansehen & bearbeiten ↗</button></article>`).join("") || '<div class="panel empty">Deine erste Kampagne beginnt mit einem Ziel.<br>Lege jetzt einen Entwurf an.</div>'}</div>`
  );
}
function products() {
  return (
    heading(
      "DAS NEUROFUNKEN ANGEBOT",
      "Ein Katalog. Viele Möglichkeiten.",
      "Deine Produkte und Leistungen als Grundlage für Vertrieb und spätere Abrechnung.",
    ) +
    `<div class="notice">Die Produktvorlagen stammen aus dem aktiven Distributionsprofil. Eigene Katalogpflege, verbindliche Preislisten und buchbare Tarife folgen.</div><div class="card-grid">${state.data.catalog.map((p) => `<article class="panel catalog-card"><span class="badge">${esc(p.category)}</span><h2>${esc(p.name)}</h2><p>${esc(p.description)}</p><div class="card-detail">Geplantes Abrechnungsmodell<strong>${esc(p.billing)}</strong></div></article>`).join("")}</div>`
  );
}
function agents() {
  return (
    heading(
      "GEMEINSAM ARBEITEN",
      "Für Agenten gebaut.",
      "Strukturierte Befehle, nachvollziehbare Änderungen und wiederaufnehmbare Abläufe.",
      `<a class="button" href="/api/v1/openapi.json" target="_blank" rel="noopener">OpenAPI ↗</a><a class="button primary" href="/api/v1/agent/context" target="_blank" rel="noopener">Kontext öffnen ↗</a>`,
    ) +
    `<div class="notice">${esc(state.data.distribution.agent_note)}</div><div class="main-grid"><section class="panel">${panelHeader("Ein Vertrag für Mensch und Agent", "Alle Änderungen nutzen dieselbe Fachlogik.")}<div class="panel-body"><pre class="code-box">GET /api/v1/agent/context
GET /api/v1/events?after=0

POST /api/v1/leads
Idempotency-Key: eindeutiger-befehl
Authorization: Bearer &lt;agent-token&gt;

PATCH /api/v1/leads/{id}
{ "version": 1, "status": "qualified" }</pre></div></section><section class="panel agent-panel">${panelHeader("Klare Aufgaben")}<div class="panel-body">${
      state.data.leads
        .filter((l) => !["won", "lost"].includes(l.status))
        .slice(0, 3)
        .map(
          (l) =>
            `<div class="activity"><span class="activity-dot"></span><div><p><strong>${esc(l.company)}</strong></p><p>${esc(l.next_action)}</p><small>Version ${l.version} · ${stages[l.status]}</small></div></div>`,
        )
        .join("") || '<p class="empty">Noch keine offenen Aufgaben.</p>'
    }</div></section></div><section class="panel">${panelHeader("Aktivitätsverlauf", "Arbeitsinstanzen verwenden die geprüfte Benutzer- oder Agentenidentität; die Demo verwendet lokale Labels.")}<div class="table-scroll"><table class="event-table"><thead><tr><th>Zeitpunkt</th><th>Aktion</th><th>Urheber</th><th>Ausführung</th></tr></thead><tbody>${state.events.map((e) => `<tr><td>${esc(date(e.created_at))}</td><td>${esc(e.summary)}</td><td>${esc(e.actor_label)}</td><td><span class="badge">Lokal gespeichert</span></td></tr>`).join("")}</tbody></table></div></section>`
  );
}
function integrations() {
  return (
    heading(
      "VERBINDUNGEN, DIE WEITERBRINGEN",
      "Dein Ökosystem wächst mit.",
      state.data.distribution.description,
      btn("Website-Import", "import", true, "import"),
    ) +
    `<div class="notice">DATEV und SAP gehören zum geplanten Kundenangebot. Pro Kunde wird festgelegt, welches System für Stammdaten, Aufträge und Buchungen führend ist. Die Karten zeigen den tatsächlichen Umsetzungsstand.</div><div class="card-grid">${state.data.integrations.map((i) => `<article class="panel integration-card"><div class="integration-mark">${esc(i.name.slice(0, 2))}</div><h2>${esc(i.name)}</h2><p>${esc(i.description)}</p><div class="integration-state"><span>${esc(i.category)}</span><span>${i.status === "local_import" ? "◉ Lokaler Import verfügbar" : "○ Geplant"}</span></div></article>`).join("")}</div>`
  );
}
function roadmap() {
  const phases = [
    [
      "01",
      "Vertrieb & Kampagnen",
      "Lokal verfügbar",
      "Leads anlegen, Phasen wechseln, Kampagnen planen, Website-Anfragen importieren und über dieselbe API mit Agenten arbeiten.",
    ],
    [
      "02",
      "Angebot → Vertrag → Betrieb",
      "Nächster Ausbau",
      "Kundenstamm, versionierte Angebote, Verträge, Abonnements und Produktberechtigungen. Support und Provisionierung über optionale Adapter; für NeuroFunken mit eigener Verdrahtung.",
    ],
    [
      "03",
      "Belege, Zahlungen & Buchhaltung",
      "Geplant",
      "Rechnungen und E-Rechnungen, Eingangsbelege, Konten und Buchungssätze. Mollie-Zahlungsabgleich, Mahnungen, DATEV-Export und Datenservices; SAP-Anbindung nach Kundenprofil.",
    ],
    [
      "04",
      "Steuerarbeitsraum & Unternehmenssteuerung",
      "Geplant",
      "Belege und Buchungen mit Steuerperioden verbinden. Exporte und Steuererklärungen vorbereiten, später optional über ERiC übermitteln. Controlling und Kapazitätsplanung aus belegten Daten.",
    ],
    [
      "05",
      "Dein Produkt. Dein Betrieb.",
      "MIT · Helm · SaaS",
      "Eigenständiges MIT-Produkt und zugeschnittene NeuroFunken-Distribution auf gemeinsamem Kern. Self-Hosting, betreuter Kundencluster oder SaaS sind unabhängig davon wählbare Betriebsformen.",
    ],
  ];
  return (
    heading(
      "VOM SALES-WERKZEUG ZUM ERP",
      "Ein Fundament fürs Ganze.",
      "Klein starten. Fachlich sauber wachsen. Mit offenen Schnittstellen.",
      `<a class="button" href="/concept" target="_blank" rel="noopener">Produktkonzept ↗</a>`,
    ) +
    `<div class="roadmap">${phases.map(([n, title, status, text]) => `<section class="panel roadmap-step"><span class="roadmap-number">${n}</span><div><div class="eyebrow">${status}</div><h2>${title}</h2><p>${text}</p></div></section>`).join("")}</div>`
  );
}
function distribution() {
  const active = state.data.distribution.id;
  return heading("BRILLIANTIS · MIT", "Ein Produkt. Zwei Ausprägungen.", "Eigenständig einsetzen oder tief ins NeuroFunken-Ökosystem einbinden.") +
    `<div class="distribution-grid"><section class="panel ${active === "standalone" ? "selected" : ""}"><span class="badge">${active === "standalone" ? "Aktives Profil" : "Eigenständig"}</span><h2>Brilliantis Standalone</h2><p>Das unabhängige CRM und ERP: eigene Produkte, eigene Prozesse, eigene Infrastruktur. Offene APIs für Menschen und Agenten; Identität, Secrets, Support und Agentenlaufzeit frei wählbar.</p><p>Der geplante kaufmännische Funktionsumfang gehört zum MIT-Produkt: von Vertrieb und Kampagnen bis zu Banking, Shops, E-Rechnungen, DATEV und SAP.</p></section><section class="panel ${active === "neurofunken" ? "selected" : ""}"><span class="badge">${active === "neurofunken" ? "Aktives Profil" : "Zugeschnitten"}</span><h2>Brilliantis für NeuroFunken</h2><p>Unsere integrierte Geschäftsplattform: Website → Lead → Vertrag → Hosting oder SaaS → Abrechnung → SolveIT-Service. Agenten arbeiten im bestehenden Symphony-Betrieb.</p><p>Eigene Produktmodelle, Organisationszuordnungen und Prozesse verbinden Keycloak, Bitwarden, SolveIT, FireFlint, Kundencenter und GitOps. Diese Verdrahtung wird separat gepflegt.</p></section></div>
    <section class="panel"><div class="panel-body"><div class="eyebrow">SERVICE NACH BEDARF</div><h2>So viel Unterstützung, wie du brauchst.</h2><p>Geplantes Angebot: SaaS und privates Hosting, Betrieb im Kundencluster, Einrichtung und Migration, individuelle Schnittstellen, Agentenbetrieb, Updates, Backup, Monitoring, Schulung und Support.</p><p>Die Distribution bestimmt Prozesse und Integrationen. Die Betriebsform bestimmt, wer die Installation betreibt. Auch eigenständiges Brilliantis kann durch NeuroFunken betreut werden.</p></div></section>
    <div class="notice">Umgesetzt: Kunden, Angebote, Verträge, Rechnungen, Finanzjournal, Marketing, Anmeldung und konfigurierbare Schnittstellen. Der automatische Betrieb über alle NeuroFunken-Systeme und die Anbieterabnahme folgen schrittweise.</div>`;
}
const creatorKinds = {
  brand: "Eigenes Markenprofil",
  virtual: "Virtuelles Profil",
  external: "Externer Influencer",
};
const creatorStatuses = {
  planned: "Geplant",
  briefing: "Im Briefing",
  collaborating: "In Zusammenarbeit",
  paused: "Pausiert",
};
const creatorChannels = {
  instagram: "Instagram",
  tiktok: "TikTok",
  youtube: "YouTube",
  linkedin: "LinkedIn",
  forum: "Forum",
  blog: "Blog",
  other: "Weiterer Kanal",
};
function influencers() {
  return (
    heading(
      "WENIGE PROFILE. EINE KLARE STIMME.",
      "Menschen, Marken & Geschichten.",
      "Eigene Profile und externe Influencer gemeinsam im Vertrieb steuern.",
      btn("Profil anlegen", "new-creator", true),
    ) +
    demoNote() +
    `<p class="section-intro">Verwalte Themen, Kanäle, Kontakte und Kooperationsbudgets. Redaktionsplan, Kanalaccounts und Kooperationen stehen in den Marketing-Ansichten bereit. Externe Veröffentlichung benötigt einen konfigurierten Anbieterzugang.</p><div class="card-grid">${state.data.creators.map((c) => `<article class="panel campaign-card"><span class="badge">${esc(creatorKinds[c.kind])}</span><h2>${esc(c.name)}</h2><p>${esc(c.niche)}</p><dl><div><dt>Kanäle</dt><dd>${c.channels.map((k) => esc(creatorChannels[k])).join(" · ")}</dd></div><div><dt>Budgetrahmen</dt><dd>${money(c.budget_cents)}</dd></div><div><dt>Zusammenarbeit</dt><dd>${esc(creatorStatuses[c.status])}</dd></div></dl><button class="button" data-creator="${esc(c.id)}">Profil & Kooperation öffnen ↗</button></article>`).join("") || '<div class="panel empty">Lege ein eigenes Profil oder eine externe Kooperation an.</div>'}</div>`
  );
}
function creatorForm() {
  openDialog(
    "Ein Profil für deine Kampagnen",
    `<p>Ein lokales Profil hält deine Planung zusammen. Externe Konten werden später über den jeweiligen Anbieter verbunden.</p><form id="creator-form"><div class="form-grid"><label class="full">Name<input name="name" required maxlength="160" autofocus></label><label>Profilart<select name="kind">${Object.entries(
      creatorKinds,
    )
      .map(([k, v]) => `<option value="${k}">${v}</option>`)
      .join(
        "",
      )}</select></label><label>Erster Kanal<select name="channels">${Object.entries(
      creatorChannels,
    )
      .map(([k, v]) => `<option value="${k}">${v}</option>`)
      .join(
        "",
      )}</select></label><label class="full">Thema & Ausrichtung<input name="niche" required maxlength="500"></label><label>Kontakt-E-Mail<input name="email" type="email" maxlength="254"></label><label>Budgetrahmen in EUR<input name="budget" type="number" min="0" max="1000000000" step="0.01" value="0" required></label><label class="full">Handles / öffentliche Profilreferenzen<input name="handles" maxlength="1000" placeholder="Zum Beispiel Instagram: @deinprofil"></label><label class="full">Briefing & Kooperationsnotizen<textarea name="notes" maxlength="2000"></textarea></label></div>${formEnd("Profil anlegen")}`,
  );
}
function creatorDetail(id) {
  const c = state.data.creators.find((c) => c.id === id);
  if (!c) return;
  const allowed = {
    planned: ["planned", "briefing", "paused"],
    briefing: ["briefing", "collaborating", "paused"],
    collaborating: ["collaborating", "paused"],
    paused: ["paused", "planned", "briefing"],
  };
  openDialog(
    esc(c.name),
    `<p>${esc(creatorKinds[c.kind])} · ${esc(c.niche)}</p><dl class="detail-grid"><div><dt>Kanäle</dt><dd>${c.channels.map((k) => esc(creatorChannels[k])).join(" · ")}</dd></div><div><dt>Budgetrahmen</dt><dd>${money(c.budget_cents)}</dd></div><div><dt>Profilreferenzen</dt><dd>${esc(c.handles || "Noch nicht hinterlegt")}</dd></div><div><dt>Kontakt</dt><dd>${esc(c.email || "Noch nicht hinterlegt")}</dd></div></dl><div class="detail-note">${esc(c.notes || "Noch keine Notizen.")}</div><form id="creator-update" data-id="${esc(id)}" data-version="${c.version}"><div class="form-grid"><label class="full">Kooperationsstatus<select name="status">${allowed[c.status].map((k) => `<option value="${k}">${creatorStatuses[k]}</option>`).join("")}</select></label></div>${formEnd("Status speichern")}`,
  );
}
function render() {
  if (!state.data) return;
  const visibleNav = nav.filter(([key]) => key !== "settings" || window.BrilliantisAccess.demo || window.BrilliantisAccess.user?.role === "admin");
  $("#nav").innerHTML = visibleNav
    .map(
      ([id, label]) =>
        `${({invoices:"FINANZEN",campaigns:"MARKETING",integrations:"PLATTFORM"})[id] ? `<div class="nav-separator"></div><p class="nav-label">${({invoices:"FINANZEN",campaigns:"MARKETING",integrations:"PLATTFORM"})[id]}</p>` : ""}<a href="#${id}" class="${state.view === id ? "active" : ""}" ${state.view === id ? 'aria-current="page"' : ""}>${icon(id)}${label}${id === "pipeline" ? `<span class="nav-count">${state.data.metrics.open_leads}</span>` : ""}</a>`,
    )
    .join("");
  $("#breadcrumb").textContent =
    nav.find(([id]) => id === state.view)?.[1] || "Übersicht";
  const screens = {
    overview,
    pipeline,
    campaigns,
    influencers,
    products,
    agents,
    integrations,
    roadmap,
    distribution,
  };
  if (window.Workspace?.screens[state.view]) window.Workspace.screens[state.view]();
  else $("#workspace").innerHTML = (screens[state.view] || overview)();
  document.title = `${nav.find(([id]) => id === state.view)?.[1] || "Übersicht"} · Brilliantis`;
}
function navigate() {
  const id = location.hash.slice(1);
  state.view = nav.some(([key]) => key === id) ? id : "overview";
  render();
  $("#sidebar").classList.remove("open");
  $("#menu-toggle").setAttribute("aria-expanded", "false");
}
const productOptions = (id) =>
  state.data.catalog
    .map(
      (p) =>
        `<option value="${p.id}" ${p.id === id ? "selected" : ""}>${esc(p.name)}</option>`,
    )
    .join("");
function openDialog(title, content) {
  $("#dialog-content").innerHTML =
    `<div class="dialog-head"><h2 id="dialog-title">${title}</h2><button class="icon-button" data-close aria-label="Dialog schließen">${icon("close")}</button></div><div class="dialog-body">${content}</div>`;
  $("#editor").classList.remove("wide");
  if (!$("#editor").open) $("#editor").showModal();
}
function formEnd(label = "Speichern") {
  return `<p class="form-error" role="alert"></p><div class="form-actions"><button type="button" class="button" data-close>Abbrechen</button><button type="submit" class="button primary">${label}</button></div></form>`;
}
function leadForm() {
  openDialog(
    "Eine neue Verkaufschance",
    `<p>Halte das Interesse fest und gib dem nächsten Schritt eine Richtung.</p><form id="lead-form"><div class="form-grid"><label>Unternehmen<input name="company" required maxlength="160" autofocus></label><label>Kontaktperson<input name="contact" maxlength="120"></label><label class="full">Vorhaben<input name="title" required maxlength="200" placeholder="Was möchte das Unternehmen erreichen?"></label><label>E-Mail<input name="email" type="email" maxlength="254"></label><label>Produkt<select name="product_id">${productOptions("hosting")}</select></label><label>Geschätzter Auftragswert in EUR netto<input name="amount" type="number" min="0" max="1000000000" step="0.01" value="0" required></label><label>Quelle<select name="source">${Object.entries(
      sources,
    )
      .map(([k, v]) => `<option value="${k}">${v}</option>`)
      .join(
        "",
      )}</select></label><label class="full">Nächster Schritt<textarea name="next_action" required maxlength="500" placeholder="Zum Beispiel: Bedarfsgespräch vereinbaren"></textarea></label></div>${formEnd("Lead anlegen")}`,
  );
}
function campaignForm() {
  openDialog(
    "Deine nächste Kampagne",
    `<p>Ein gutes Briefing ist der erste Schritt. Gespeichert wird ein Entwurf ohne externen Versand.</p><form id="campaign-form"><div class="form-grid"><label class="full">Kampagnenname<input name="name" required maxlength="160" autofocus></label><label>Produkt<select name="product_id">${productOptions("hosting")}</select></label><label>Kanal<select name="channel">${Object.entries(
      channels,
    )
      .map(([k, v]) => `<option value="${k}">${v}</option>`)
      .join(
        "",
      )}</select></label><label class="full">Zielgruppe<input name="audience" required maxlength="500" placeholder="Wen möchtest du erreichen?"></label><label class="full">Ziel & Botschaft<textarea name="objective" required maxlength="1000"></textarea></label><label>Budgetrahmen in EUR<input name="budget" type="number" min="0" max="1000000000" step="0.01" value="0" required></label></div>${formEnd("Entwurf speichern")}`,
  );
}
function leadDetail(id) {
  const l = state.data.leads.find((l) => l.id === id);
  if (!l) return;
  const allowed = {
    new: ["new", "qualified", "lost"],
    qualified: ["qualified", "new", "proposal", "lost"],
    proposal: ["proposal", "qualified", "won", "lost"],
    won: ["won"],
    lost: ["lost"],
  };
  openDialog(
    esc(l.company),
    `<div class="detail-title">${esc(l.title)}</div><dl class="detail-grid"><div><dt>Kontakt</dt><dd>${esc(l.contact || "Noch nicht hinterlegt")}<br>${esc(l.email)}</dd></div><div><dt>Produkt & Volumen</dt><dd>${esc(productName(l.product_id))}<br>${money(l.amount_cents)} netto</dd></div><div><dt>Quelle</dt><dd>${esc(sources[l.source])}</dd></div><div><dt>Stand</dt><dd>Version ${l.version} · ${esc(date(l.updated_at))}</dd></div></dl>${l.inquiry ? `<div class="detail-note">${esc(l.inquiry.message)}<br><small>Hosting-Kalkulation als unverbindlicher Snapshot. Keine Marketingfreigabe aus Kontaktzustimmung.</small></div>` : ""}<form id="lead-update" data-id="${esc(id)}" data-version="${l.version}"><div class="form-grid"><label class="full">Vertriebsphase<select name="status">${allowed[l.status].map((s) => `<option value="${s}">${stages[s]}</option>`).join("")}</select></label><label class="full">Nächster Schritt<textarea name="next_action" required maxlength="500">${esc(l.next_action)}</textarea></label></div>${formEnd("Änderung speichern")}`,
  );
}
function campaignDetail(id) {
  const c = state.data.campaigns.find((c) => c.id === id);
  if (!c) return;
  const allowed = {
    draft: ["draft", "planned"],
    planned: ["planned", "draft", "paused"],
    paused: ["paused", "planned", "draft"],
  };
  openDialog(
    esc(c.name),
    `<p>${esc(c.objective)}</p><dl class="detail-grid"><div><dt>Zielgruppe</dt><dd>${esc(c.audience)}</dd></div><div><dt>Produkt</dt><dd>${esc(productName(c.product_id))}</dd></div><div><dt>Kanal</dt><dd>${esc(channels[c.channel])}</dd></div><div><dt>Budgetrahmen</dt><dd>${money(c.budget_cents)}</dd></div></dl><form id="campaign-update" data-id="${esc(id)}" data-version="${c.version}"><div class="form-grid"><label class="full">Planungsstatus<select name="status">${allowed[c.status].map((s) => `<option value="${s}">${campaignStates[s]}</option>`).join("")}</select></label></div><p class="section-intro">Der Statuswechsel plant die Kampagne lokal. Es werden keine Nachrichten verschickt.</p>${formEnd("Plan speichern")}`,
  );
}
function importForm() {
  openDialog(
    "Website-Anfrage importieren",
    `<p>Wähle eine einzelne Anfrage als JSON-Datei aus dem Website-Export. Sie wird nur in dieser lokalen Brilliantis-Instanz gespeichert. Ein erneuter Import derselben Anfrage wird erkannt.</p><form id="import-form"><div class="form-grid"><label class="full">Anfrage als JSON<input name="file" type="file" accept="application/json,.json" required></label></div>${formEnd("Lokal importieren")}`,
  );
}
document.addEventListener("click", (event) => {
  const close = event.target.closest("[data-close]");
  if (close) {
    $("#editor").close();
    return;
  }
  const nav = event.target.closest("[data-nav]");
  if (nav) {
    location.hash = nav.dataset.nav;
    return;
  }
  const lead = event.target.closest("[data-lead]");
  if (lead) {
    leadDetail(lead.dataset.lead);
    return;
  }
  const creator = event.target.closest("[data-creator]");
  if (creator) {
    creatorDetail(creator.dataset.creator);
    return;
  }
  const campaign = event.target.closest("[data-campaign]");
  if (campaign) {
    campaignDetail(campaign.dataset.campaign);
    return;
  }
  const action = event.target.closest("[data-action]")?.dataset.action;
  if (action === "new-creator") creatorForm();
  if (action === "new-lead") leadForm();
  if (action === "new-campaign") campaignForm();
  if (action === "import") importForm();
});
document.addEventListener("input", (event) => {
  if (event.target.id === "lead-search") {
    const pos = event.target.selectionStart;
    state.query = event.target.value;
    render();
    const input = $("#lead-search");
    input.focus();
    if (pos !== null) input.setSelectionRange(pos, pos);
  }
});
document.addEventListener("change", (event) => {
  if (event.target.id === "product-filter") {
    state.filter = event.target.value;
    render();
  }
});
document.addEventListener("submit", async (event) => {
  const form = event.target;
  if (
    ![
      "lead-form",
      "campaign-form",
      "lead-update",
      "campaign-update",
      "creator-form",
      "creator-update",
      "import-form",
    ].includes(form.id)
  )
    return;
  event.preventDefault();
  const button = $("button[type=submit]", form);
  button.disabled = true;
  $(".form-error", form).textContent = "";
  try {
    const fields = Object.fromEntries(new FormData(form));
    let path,
      method = "POST",
      data;
    if (form.id === "lead-form") {
      const { amount, ...rest } = fields;
      data = { ...rest, amount_cents: Math.round(Number(amount) * 100) };
      path = "/api/v1/leads";
    }
    if (form.id === "creator-form") {
      const { budget, channels, ...rest } = fields;
      data = {
        ...rest,
        channels: [channels],
        budget_cents: Math.round(Number(budget) * 100),
      };
      path = "/api/v1/creators";
    }
    if (form.id === "campaign-form") {
      const { budget, ...rest } = fields;
      data = { ...rest, budget_cents: Math.round(Number(budget) * 100) };
      path = "/api/v1/campaigns";
    }
    if (form.id.endsWith("-update")) {
      method = "PATCH";
      path = `/api/v1/${form.id === "lead-update" ? "leads" : form.id === "creator-update" ? "creators" : "campaigns"}/${form.dataset.id}`;
      data = { ...fields, version: Number(form.dataset.version) };
    }
    if (form.id === "import-form") {
      if (fields.file.size > 60000)
        throw new Error("Bitte eine einzelne Anfrage unter 60 KB auswählen.");
      const input = JSON.parse(await fields.file.text());
      data = {
        id: input.id,
        created_at: input.createdAt,
        name: input.name,
        email: input.email,
        company: input.company || "",
        topic: input.topic,
        message: input.message,
        consent: input.consent,
        hosting: input.hosting || null,
      };
      path = "/api/v1/import/website";
    }
    await command(path, method, data);
    $("#editor").close();
    toast("Gespeichert. Brilliantis ist auf dem aktuellen Stand.");
    try {
      await load();
      render();
    } catch {
      toast(
        "Gespeichert. Die Ansicht konnte nicht aktualisiert werden; bitte neu laden.",
      );
    }
  } catch (error) {
    $(".form-error", form).textContent = error.message;
    button.disabled = false;
  }
});
$("#menu-toggle").addEventListener("click", () => {
  const open = $("#sidebar").classList.toggle("open");
  $("#menu-toggle").setAttribute("aria-expanded", String(open));
});
addEventListener("hashchange", navigate);
window.BrilliantisAccess.start()
  .then(load)
  .then(navigate)
  .catch((error) => {
    $("#workspace").innerHTML =
      `<div class="error-panel"><h2>Brilliantis konnte nicht geladen werden.</h2><p>${esc(error.message)}</p><p>Bitte prüfe den lokalen Server und lade die Seite erneut.</p></div>`;
  });

$("#editor").addEventListener("close",()=> { $("#dialog-content").replaceChildren(); });
