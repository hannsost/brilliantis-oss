import assert from 'node:assert/strict';
import { checkBriefings } from './check-briefings.mjs';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, writeFile, rm, mkdir } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { tmpdir } from 'node:os';
import path from 'node:path';
const { chromium } = await import(process.env.BRILLIANTIS_PLAYWRIGHT_MODULE || 'playwright-core');
const binary = path.resolve(process.env.BRILLIANTIS_TEST_BINARY || 'target/debug/brilliantis');
const temp = await mkdtemp(path.join(tmpdir(), 'brilliantis-access-'));
const database = path.join(temp, 'workspace.sqlite');
const passwordFile = path.join(temp, 'password');
const password = randomUUID();
let server, browser;
async function stop(child) {
  if (child?.pid && child.exitCode === null && child.signalCode === null) { const exited=once(child,'exit'); if(child.kill('SIGINT')) await exited; }
}
try {
  await writeFile(passwordFile, password, {mode:0o600});
  const init = spawn(binary, ['--database', database, '--init-admin', 'admin@example.invalid', '--password-file', passwordFile], {stdio:'ignore'});
  assert.equal((await once(init, 'exit'))[0], 0, 'Administrator bootstrap');
  await rm(passwordFile);
  server = spawn(binary, ['--database', database, '--port', '0'], {stdio:['ignore','pipe','pipe']});
  const origin = await new Promise((resolve,reject) => {
    const timer = setTimeout(()=>reject(new Error('Local server did not start')),10000);
    server.stdout.on('data', chunk => { const found=String(chunk).match(/http:\/\/127\.0\.0\.1:\d+/); if(found){clearTimeout(timer);resolve(found[0]);} });
    server.on('error',error=>{clearTimeout(timer);reject(error);});
    server.on('exit',code=>{clearTimeout(timer);reject(new Error(`Server exit ${code}`));});
  });
  const errors=[];
  browser = await chromium.launch({executablePath:process.env.BRILLIANTIS_CHROMIUM || '/usr/bin/chromium',headless:true,args:['--no-sandbox']});
  const page=await browser.newPage({viewport:{width:1440,height:1000}});
  page.on('pageerror',error=>errors.push(error.message));
  page.setDefaultTimeout(15000);
  await page.goto(origin);
  await page.getByRole('heading',{name:'Willkommen zurück.'}).waitFor();
  assert.equal((await fetch(`${origin}/api/v1/dashboard`)).status,401);
  await mkdir('artifacts',{recursive:true});
  await page.screenshot({path:'artifacts/workspace-login.png',fullPage:true});
  await page.getByLabel('E-Mail',{exact:true}).fill('admin@example.invalid');
  await page.getByLabel('Passwort',{exact:true}).fill(password);
  await page.getByRole('button',{name:'Anmelden',exact:true}).click();
  await page.getByRole('heading',{name:'Gute Ideen. Nächster Schritt.'}).waitFor();
  await checkBriefings(page,origin,'workspace');
  await page.goto(`${origin}/#access`);
  await page.getByRole('button',{name:'Zugang erstellen',exact:true}).click();
  await page.locator('dialog [name="name"]').fill('Browser agent');
  await page.locator('dialog [name="scopes"]').fill('crm:read');
  await page.locator('dialog button[type="submit"]').click();
  const token=await page.getByLabel('Token',{exact:true}).inputValue();
  assert.ok(token.startsWith('brl_'));
  assert.equal((await fetch(`${origin}/api/v1/records/customers`,{headers:{Authorization:`Bearer ${token}`}})).status,200);
  assert.equal((await fetch(`${origin}/api/v1/finance/report`,{headers:{Authorization:`Bearer ${token}`}})).status,403);
  await page.getByRole('button',{name:'Erledigt',exact:true}).click();
  assert.equal(await page.locator('.token-value').count(),0,'Token removed from DOM after closing');
  const row=page.locator('tr').filter({hasText:'Browser agent'});
  await row.getByRole('button',{name:'Widerrufen',exact:true}).click();
  await row.getByText('Widerrufen',{exact:true}).waitFor();
  assert.equal((await fetch(`${origin}/api/v1/records/customers`,{headers:{Authorization:`Bearer ${token}`}})).status,401);
  await page.getByRole('button',{name:'Abmelden',exact:true}).click();
  await page.getByRole('heading',{name:'Willkommen zurück.'}).waitFor();
  const status=await page.evaluate(async()=> (await fetch('/api/v1/dashboard')).status);
  assert.equal(status,401);
  assert.deepEqual(errors,[]);
  console.log('Workspace browser access passed: bootstrap, login, scoped token, one-time display, revocation and logout.');
} finally {
  if(browser) await browser.close();
  await stop(server);
  await rm(temp,{recursive:true,force:true});
}
