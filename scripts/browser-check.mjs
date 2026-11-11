import assert from "node:assert/strict";
import { checkBriefings } from "./check-briefings.mjs";
import { checkGrossEditor } from "./check-gross-editor.mjs";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

// The app itself needs neither Node nor Playwright. This is optional UI QA tooling.
const { chromium } = await import(
  process.env.BRILLIANTIS_PLAYWRIGHT_MODULE || "playwright-core"
);
const temp = await mkdtemp(path.join(tmpdir(), "brilliantis-browser-"));
const profile = process.env.BRILLIANTIS_TEST_PROFILE || "standalone";
const binary = path.resolve(
  process.env.BRILLIANTIS_TEST_BINARY || `target/debug/${profile === "neurofunken" ? "brilliantis-neurofunken" : "brilliantis"}`,
);
const server = spawn(
  binary,
  ["--profile", profile, "--demo", "--port", "0", "--database", path.join(temp, "demo.sqlite")],
  { stdio: ["ignore", "pipe", "pipe"] },
);
let browser;
const errors = [];
try {
  const origin = await new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error("Local server did not start")),
      10000,
    );
    server.stdout.on("data", (chunk) => {
      const found = String(chunk).match(/http:\/\/127\.0\.0\.1:\d+/);
      if (found) {
        clearTimeout(timeout);
        resolve(found[0]);
      }
    });
    server.on("error", (error) => { clearTimeout(timeout); reject(error); });
    server.on("exit", (code) => {
      clearTimeout(timeout);
      reject(new Error(`Server exit ${code}`));
    });
    server.stderr.on("data", (chunk) => errors.push(String(chunk)));
  });
  browser = await chromium.launch({
    executablePath: process.env.BRILLIANTIS_CHROMIUM || "/usr/bin/chromium",
    headless: true,
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1080 },
  });
  page.setDefaultTimeout(15000);
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  await page.goto(origin);
  await page
    .getByRole("heading", { name: "Gute Ideen. Nächster Schritt." })
    .waitFor();
  await mkdir("artifacts", { recursive: true });
  await page.screenshot({
    path: `artifacts/${profile}-desktop.png`,
    fullPage: true,
  });
  assert.match(await page.title(), /Brilliantis/);
  await checkGrossEditor(page);
  assert.equal(await page.locator("#distribution-label").textContent(), profile === "neurofunken" ? "für NeuroFunken" : "Standalone · MIT");
  console.log(`Dashboard rendered: ${profile}.`);
  await page.getByRole("button", { name: "Neuer Lead", exact: true }).click();
  await page
    .getByLabel("Unternehmen", { exact: true })
    .fill("Browser Test <img src=x onerror=alert(1)>");
  await page
    .getByLabel("Vorhaben", { exact: true })
    .fill("Persistenz und sichere Darstellung");
  await page
    .getByLabel("Geschätzter Auftragswert in EUR netto")
    .fill("1234.56");
  await page
    .getByLabel("Nächster Schritt", { exact: true })
    .fill("Bedarfsgespräch vorbereiten");
  await page.getByRole("button", { name: "Lead anlegen", exact: true }).click();
  await page.locator("dialog").waitFor({ state: "hidden" });
  await page.getByRole("link", { name: /Sales Pipeline/ }).click();
  await page.getByLabel("Leads durchsuchen").fill("Browser Test");
  const card = page.locator(".lead-card").filter({ hasText: "Browser Test" });
  await card.waitFor();
  assert.equal(await page.locator(".lead-card img").count(), 0);
  await card.click();
  await page.getByLabel("Vertriebsphase").selectOption("qualified");
  await page.getByRole("button", { name: "Änderung speichern" }).click();
  await page.locator("dialog").waitFor({ state: "hidden" });
  await page.reload();
  await page
    .locator(".lead-card")
    .filter({ hasText: "Browser Test" })
    .waitFor();
  await page
    .locator(".board-column")
    .filter({
      has: page.locator(".column-heading", { hasText: "Qualifiziert" }),
    })
    .getByRole("button", { name: /Browser Test/ })
    .waitFor();
  console.log("Lead persisted and qualified.");
  await page.getByRole("link", { name: "Kampagnen", exact: true }).click();
  await page
    .getByRole("button", { name: "Neue Kampagne", exact: true })
    .click();
  await page.getByLabel("Kampagnenname").fill("SEO Browser Test");
  await page.getByLabel("Kanal").selectOption("seo");
  await page
    .getByLabel("Zielgruppe", { exact: true })
    .fill("Kleine Softwareteams");
  await page.getByLabel("Ziel & Botschaft").fill("Passende Hosting-Anfragen");
  await page.getByRole("button", { name: "Entwurf speichern" }).click();
  await page.locator("dialog").waitFor({ state: "hidden" });
  await page.getByRole("heading", { name: "SEO Browser Test" }).waitFor();
  console.log("SEO campaign created.");
  await page.getByRole("link", { name: "Influencer", exact: true }).click();
  await page.getByRole("button", { name: "Profil anlegen" }).click();
  await page.getByLabel("Name", { exact: true }).fill("Externer Creator Test");
  await page.getByLabel("Profilart").selectOption("external");
  await page.getByLabel("Thema & Ausrichtung").fill("Offene Software");
  await page.getByLabel("Erster Kanal").selectOption("youtube");
  await page
    .getByRole("button", { name: "Profil anlegen", exact: true })
    .last()
    .click();
  await page.locator("dialog").waitFor({ state: "hidden" });
  const creator = page
    .locator(".campaign-card")
    .filter({ hasText: "Externer Creator Test" });
  await creator.getByRole("button").click();
  await page.getByLabel("Kooperationsstatus").selectOption("briefing");
  await page.getByRole("button", { name: "Status speichern" }).click();
  await page.locator("dialog").waitFor({ state: "hidden" });
  await page
    .locator("#workspace")
    .getByText("Im Briefing", { exact: true })
    .waitFor();
  console.log("External creator saved.");
  await page.getByRole("link", { name: /Sales Pipeline/ }).click();
  await page.getByRole("button", { name: "Website-Import" }).click();
  const inquiry = {
    id: "browser-inquiry",
    createdAt: "2026-10-07T12:00:00Z",
    name: "Import Beispiel",
    email: "import@example.invalid",
    company: "Browser Import",
    topic: "betrieb",
    message: "Ein kontrollierter Hosting-Import als Test.",
    consent: true,
    hosting: { quote: { monthlyCents: 50000 } },
  };
  await page
    .getByLabel("Anfrage als JSON")
    .setInputFiles({
      name: "inquiry.json",
      mimeType: "application/json",
      buffer: Buffer.from(JSON.stringify(inquiry)),
    });
  await page.getByRole("button", { name: "Lokal importieren" }).click();
  await page.locator("dialog").waitFor({ state: "hidden" });
  const data = await (await fetch(`${origin}/api/v1/dashboard`)).json();
  assert.equal(data.distribution.id, profile);
  assert.equal(data.catalog.some((p) => p.id === "fireflint"), profile === "neurofunken");
  assert.equal(data.integrations.some((p) => p.id === "symphony"), profile === "neurofunken");
  assert.equal(
    data.leads.find((l) => l.company === "Browser Import").amount_cents,
    0,
  );
  assert.equal(
    data.creators.find((c) => c.name === "Externer Creator Test").status,
    "briefing",
  );
  // Complete business flow through the interface, using only synthetic data.
  await page.goto(`${origin}/#settings`);
  await page.getByLabel("Unternehmensname", {exact:true}).fill("Browser Test GmbH");
  for (const [label,value] of [["Straße und Hausnummer","Musterstraße 1"],["Postleitzahl","10115"],["Ort","Berlin"],["E-Mail","office@example.invalid"],["Zahlungsbedingungen","14 Tage netto"]]) await page.getByLabel(label,{exact:true}).fill(value);
  await page.getByRole("button",{name:"Unternehmensdaten speichern"}).click();
  await page.getByRole("status").filter({hasText:"Gespeichert"}).waitFor();
  await page.goto(`${origin}/#customers`);
  await page.getByRole("button",{name:"Geschäftspartner anlegen",exact:true}).first().click();
  await page.getByLabel("Name / Unternehmen",{exact:true}).fill("UI Partner GmbH");
  for (const [name,value] of [["address","Partnerstraße 1"],["postal_code","10115"],["city","Berlin"],["email","partner@example.invalid"]]) await page.locator(`dialog [name="${name}"]`).fill(value);
  await page.locator('dialog button[type="submit"]').click();
  await page.locator("dialog").waitFor({state:"hidden"});
  await page.locator(".record-title",{hasText:"UI Partner GmbH"}).waitFor();
  await page.goto(`${origin}/#quotes`);
  await page.getByRole("button",{name:"Angebot anlegen",exact:true}).first().click();
  await page.getByLabel("Kunde suchen",{exact:true}).fill("UI Partner");
  await page.getByRole("option",{name:/UI Partner GmbH/}).click();
  await page.getByLabel("Betreff",{exact:true}).fill("UI Service-Angebot");
  await page.getByLabel("Gültig bis",{exact:true}).fill("2099-12-31");
  await page.locator('[name="line_description"]').fill("Betrieb und Einrichtung");
  await page.locator('[name="line_quantity"]').fill("2");
  await page.locator('[name="line_price"]').fill("100");
  await page.locator('dialog button[type="submit"]').click();
  await page.locator("dialog").waitFor({state:"hidden"});
  await page.locator(".record-title",{hasText:"UI Service-Angebot"}).click();
  await page.getByRole("button",{name:"Angebot ausgeben",exact:true}).click();
  await page.getByRole("button",{name:"Annahme erfassen",exact:true}).click();
  await page.getByRole("button",{name:/Zugehörigen Vertrag öffnen/}).click();
  await page.locator('dialog .badge.active').waitFor();
  await page.getByRole("button",{name:"Dialog schließen",exact:true}).click();
  await page.goto(`${origin}/#invoices`);
  await page.getByRole("button",{name:"Rechnung anlegen",exact:true}).first().click();
  await page.getByLabel("Geschäftspartner suchen",{exact:true}).fill("UI Partner");
  await page.getByRole("option",{name:/UI Partner GmbH/}).click();
  await page.getByLabel("Betreff",{exact:true}).fill("UI Service-Rechnung");
  await page.locator('[name="line_description"]').fill("Betrieb und Einrichtung");
  await page.locator('[name="line_quantity"]').fill("2");
  await page.locator('[name="line_price"]').fill("100");
  await page.locator('dialog button[type="submit"]').click();
  await page.locator("dialog").waitFor({state:"hidden"});
  await page.locator(".record-title",{hasText:"UI Service-Rechnung"}).click();
  await page.getByRole("button",{name:"Rechnung buchen",exact:true}).click();
  await page.locator('dialog .badge.issued').waitFor();
  await page.getByRole("button",{name:"Dialog schließen",exact:true}).click();
  await page.goto(`${origin}/#finance`);
  await page.getByRole("heading",{name:"Salden & Konten",exact:true}).waitFor();
  await page.screenshot({path:`artifacts/${profile}-finance.png`,fullPage:true});
  console.log("Customer, organization, quote, contract, invoice and financial dashboard verified.");
  // A portal-priced offer must use the commerce acceptance route even from the GUI.
  async function commerce(path, body) {
    const response=await fetch(`${origin}/api/v1/commerce/${path}`,{method:'POST',headers:{'Content-Type':'application/json','X-Brilliantis-Client':'test.browser-commerce','Idempotency-Key':`browser-${path}-${crypto.randomUUID()}`},body:JSON.stringify(body)});
    const result=await response.json();assert.equal(response.status,200,JSON.stringify(result));return result.record;
  }
  await commerce('catalog',{version:0,revision:'browser-r1',items:[{id:'hosting',name:'Portal Hosting Test',kind:'hosting',charges:[{description:'Betrieb',cadence:'monthly',unit_price_cents:2000,tax_basis_points:1900}],dimensions:[]}]});
  await commerce('customers',{source:'browser-portal',external_id:'customer-1',customer:{name:'Portal Testkunde',email:'portal@example.invalid',contact:'',address:'Testweg 1',postal_code:'10115',city:'Berlin',country:'DE',vat_id:'',notes:''}});
  await commerce('offers',{source:'browser-portal',external_id:'checkout-1',external_customer_id:'customer-1',catalog_revision:'browser-r1',configuration_revision:'config-r1',item_id:'hosting',quantity:1,configuration:{}});
  await page.goto(`${origin}/#quotes`);
  await page.locator('.record-title',{hasText:'Portal Hosting Test'}).click();
  await page.getByRole('button',{name:'Annahme erfassen',exact:true}).click();
  await page.getByLabel('Nachweis der Kundenannahme',{exact:true}).fill('browser:documented-acceptance');
  await page.getByRole('button',{name:'Annahme und Auftrag speichern',exact:true}).click();
  await page.locator('dialog').waitFor({state:'hidden'});
  await page.goto(`${origin}/#orders`);
  await page.locator('.record-title',{hasText:'Portal Hosting Test'}).click();
  await page.getByRole('heading',{name:'Portalauftrag',exact:true}).waitFor();
  assert.match(await page.locator('dialog').innerText(),/Ausstehend/);
  assert.match(await page.locator('dialog').innerText(),/Noch nicht abgerechnet/);
  assert.match(await page.locator('dialog').innerText(),/Monatlich/);
  await page.getByRole('button',{name:'Dialog schließen',exact:true}).click();
  console.log('Portal offer acceptance and order billing/provisioning details verified.');
  for (const hash of [
    "overview",
    "pipeline",
    "campaigns",
    "influencers",
    "products",
    "agents",
    "integrations",
    "customers", "quotes", "contracts", "orders", "invoices", "credit-notes", "bank-accounts", "bank-transactions", "finance", "channel-accounts", "content", "collaborations", "seo-keywords", "briefings", "settings", "access",
    "distribution",
  ]) {
    await page.goto(`${origin}/#${hash}`);
    await page.locator("h1").waitFor();
    await page.locator(".loading").waitFor({state:"hidden"});
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth > innerWidth,
      ),
      false,
      `Desktop overflow on ${hash}`,
    );
  }
  await checkBriefings(page, origin, profile);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(origin);
  await page
    .getByRole("heading", { name: "Gute Ideen. Nächster Schritt." })
    .waitFor();
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth,
    ),
    false,
  );
  await page.screenshot({
    path: `artifacts/${profile}-mobile.png`,
    fullPage: true,
  });
  await page.getByRole("button", { name: "Navigation öffnen" }).click();
  await page.getByRole("link", { name: "Influencer", exact: true }).click();
  await page
    .getByRole("heading", { name: "Menschen, Marken & Geschichten." })
    .waitFor();
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth,
    ),
    false,
  );
  await page.getByRole("button", { name: "Profil anlegen" }).click();
  await page.keyboard.press("Escape");
  await page.locator("dialog").waitFor({ state: "hidden" });
  for (const hash of ["finance", "invoices", "customers", "content", "briefings", "access"]) {
    await page.goto(`${origin}/#${hash}`);
    await page.locator(".loading").waitFor({state:"hidden"});
    await page.locator("h1").waitFor();
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `Mobile overflow on ${hash}`);
  }
  await browser.close();
  browser = null;
  assert.deepEqual(errors, []);
  console.log(
    "Browser checks passed: desktop/mobile, all routes, lead create/update, SEO campaign, external creator, import, escaping, reload persistence and dialog keyboard behavior.",
  );
} finally {
  if (browser) await browser.close();
  if (server.pid && server.exitCode === null && server.signalCode === null) {
    const exited = once(server, "exit");
    if (server.kill("SIGINT")) await exited;
  }
  await rm(temp, { recursive: true, force: true });
}
