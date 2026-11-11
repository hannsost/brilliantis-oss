// Real HTTPS browser roundtrip against a local Keycloak-protocol fixture.
// All certificates, signing keys, account data and client secrets are generated per test.
import assert from 'node:assert/strict';
import {spawn,execFileSync} from 'node:child_process';
import {once} from 'node:events';
import {mkdtemp,writeFile,readFile,rm,mkdir} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import http from 'node:http';
import https from 'node:https';
import {createHash,createHmac,generateKeyPairSync,randomUUID,sign} from 'node:crypto';

const {chromium}=await import(process.env.BRILLIANTIS_PLAYWRIGHT_MODULE||'playwright-core');
const binary=path.resolve(process.env.BRILLIANTIS_TEST_BINARY||'target/debug/brilliantis');
const temp=await mkdtemp(path.join(tmpdir(),'brilliantis-oidc-'));
let app,provider,proxy,browser,scenario='valid';
let exchanges=0,logouts=0;
const providerErrors=[],pageErrors=[],codes=new Map();
const clientSecret=randomUUID();
const key=generateKeyPairSync('rsa',{modulusLength:2048});
const jwk={...key.publicKey.export({format:'jwk'}),kid:'fixture-rsa',use:'sig',alg:'RS256'};
const b64=x=>Buffer.from(typeof x==='string'?x:JSON.stringify(x)).toString('base64url');
function idToken(claims){
 const head={alg:scenario==='hmac'?'HS256':'RS256',kid:scenario==='unknown_key'?'unknown':jwk.kid,typ:'JWT'};
 const body=`${b64(head)}.${b64(claims)}`;
 const signature=scenario==='hmac'?createHmac('sha256','public-test-fixture').update(body).digest():sign('RSA-SHA256',Buffer.from(body),key.privateKey);
 if(scenario==='bad_signature')signature[0]^=1;
 return `${body}.${signature.toString('base64url')}`;
}
async function stop(child){if(child?.pid && child.exitCode===null && child.signalCode===null){const exited=once(child,'exit');if(child.kill('SIGINT'))await exited;}}
async function close(server){if(server?.listening){server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}}
async function listen(server){await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve);});return server.address().port;}
try {
 const caKey=path.join(temp,'ca.key'),ca=path.join(temp,'ca.pem'),tlsKey=path.join(temp,'tls.key'),csr=path.join(temp,'tls.csr'),cert=path.join(temp,'tls.pem'),ext=path.join(temp,'tls.ext');
 execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-days','1','-subj','/CN=Brilliantis Test CA','-keyout',caKey,'-out',ca],{stdio:'ignore'});
 execFileSync('openssl',['req','-new','-newkey','rsa:2048','-nodes','-subj','/CN=localhost','-keyout',tlsKey,'-out',csr],{stdio:'ignore'});
 await writeFile(ext,'basicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n');
 execFileSync('openssl',['x509','-req','-in',csr,'-CA',ca,'-CAkey',caKey,'-CAcreateserial','-days','1','-extfile',ext,'-out',cert],{stdio:'ignore'});
 const tls={key:await readFile(tlsKey),cert:await readFile(cert)};
 let issuer,origin;
 provider=https.createServer(tls,async(req,res)=>{
  try{
   const url=new URL(req.url,issuer);
   if(url.pathname.endsWith('/auth')){
    const p=url.searchParams;
    assert.equal(p.get('client_id'),'brilliantis');assert.equal(p.get('response_type'),'code');assert.equal(p.get('response_mode'),'query');
    assert.equal(p.get('redirect_uri'),`${origin}/api/v1/auth/oidc/callback`);assert.equal(p.get('scope'),'openid profile email');assert.equal(p.get('code_challenge_method'),'S256');
    assert.equal(p.get('state').length,64);assert.equal(p.get('nonce').length,64);assert.equal(p.get('code_challenge').length,43);
    const code=randomUUID();codes.set(code,Object.fromEntries(p));
    const target=new URL(p.get('redirect_uri'));target.searchParams.set('code',code);target.searchParams.set('state',p.get('state'));target.searchParams.set('iss',issuer);
    res.writeHead(303,{Location:target.toString()});res.end();return;
   }
   if(url.pathname.endsWith('/token')){
    assert.equal(req.method,'POST');assert.equal(req.headers.authorization,'Basic '+Buffer.from(`brilliantis:${clientSecret}`).toString('base64'));
    let body='';for await(const part of req)body+=part;const p=new URLSearchParams(body);
    const request=codes.get(p.get('code'));assert.ok(request,'unknown or replayed code');codes.delete(p.get('code'));
    assert.equal(p.get('grant_type'),'authorization_code');assert.equal(p.get('redirect_uri'),request.redirect_uri);
    assert.equal(createHash('sha256').update(p.get('code_verifier')).digest('base64url'),request.code_challenge);
    const now=Math.floor(Date.now()/1000);
    const claims={iss:issuer,aud:'brilliantis',azp:'brilliantis',sub:'verified-leon-sub',iat:now,exp:now+300,nonce:request.nonce,email:'leon@example.invalid',email_verified:true,realm_access:{roles:['admin']}};
    if(scenario==='same_email_other_sub')claims.sub='attacker-sub';
    if(scenario==='wrong_issuer')claims.iss='https://different.example.invalid';
    if(scenario==='wrong_audience')claims.aud='other-client';
    if(scenario==='wrong_nonce')claims.nonce='wrong-nonce';
    if(scenario==='expired')claims.exp=now-1;
    if(scenario==='future_iat')claims.iat=now+120;
    if(scenario==='wrong_azp')claims.azp='other-client';
    if(scenario==='missing_azp'){claims.aud=['brilliantis','another'];delete claims.azp;}
    if(scenario==='missing_nonce')delete claims.nonce;
    if(scenario==='email_changed')claims.email='renamed@example.invalid';
    exchanges++;
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({token_type:'Bearer',access_token:'fixture-unused-access-token',id_token:idToken(claims),expires_in:300}));return;
   }
   if(url.pathname.endsWith('/certs')){res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({keys:[jwk]}));return;}
   if(url.pathname.endsWith('/logout')){
    assert.equal(url.searchParams.get('client_id'),'brilliantis');assert.equal(url.searchParams.get('post_logout_redirect_uri'),`${origin}/api/v1/auth/oidc/logout/callback`);
    const target=new URL(url.searchParams.get('post_logout_redirect_uri'));target.searchParams.set('state',url.searchParams.get('state'));logouts++;
    res.writeHead(303,{Location:target.toString()});res.end();return;
   }
   res.writeHead(404);res.end();
  }catch(error){providerErrors.push(error.message);res.writeHead(500);res.end('Fixture assertion failed');}
 });
 const providerPort=await listen(provider);issuer=`https://localhost:${providerPort}/realms/test`;
 // Reserve a temporary local backend port, then put HTTPS in front as the production ingress does.
 const reservation=http.createServer();const backendPort=await listen(reservation);await close(reservation);
 proxy=https.createServer(tls,(req,res)=>{
  const upstream=http.request({host:'127.0.0.1',port:backendPort,path:req.url,method:req.method,headers:req.headers},r=>{res.writeHead(r.statusCode,r.headers);r.pipe(res);});
  upstream.on('error',()=>{res.writeHead(502);res.end();});req.pipe(upstream);
 });
 origin=`https://127.0.0.1:${await listen(proxy)}`;
 const secretPath=path.join(temp,'client-secret');await writeFile(secretPath,clientSecret,{mode:0o600});
 const configuration=path.join(temp,'oidc.json');await writeFile(configuration,JSON.stringify({issuer,audience:'brilliantis',client_id:'brilliantis',client_secret:{provider:'file',path:secretPath},browser_login:true,login_label:'Mit Test-SSO anmelden',ca_certificate_file:ca,bootstrap_admin:{subject:'verified-leon-sub',email:'leon@example.invalid',name:'Leon'}}));
 app=spawn(binary,['--database',path.join(temp,'workspace.sqlite'),'--port',String(backendPort),'--public-url',origin,'--oidc-file',configuration],{stdio:['ignore','pipe','pipe']});
 await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(new Error('App startup timeout')),10000);app.stdout.on('data',chunk=>{if(String(chunk).includes(origin)){clearTimeout(timer);resolve();}});app.on('exit',code=>{clearTimeout(timer);reject(new Error(`App exit ${code}`));});app.on('error',reject);});
 browser=await chromium.launch({executablePath:process.env.BRILLIANTIS_CHROMIUM||'/usr/bin/chromium',headless:true,args:['--no-sandbox']});
 const context=await browser.newContext({ignoreHTTPSErrors:true});const page=await context.newPage();page.setDefaultTimeout(15000);page.on('pageerror',e=>pageErrors.push(e.message));
 await page.goto(origin);
 await page.getByRole('link',{name:'Mit Test-SSO anmelden',exact:true}).click();
 await page.getByRole('heading',{name:'Gute Ideen. Nächster Schritt.'}).waitFor();
 const session=await page.evaluate(async()=>await(await fetch('/api/v1/auth/session')).json());
 assert.equal(session.user.role,'admin');assert.equal(session.user.name,'Leon');assert.ok(session.csrf);
 const cookies=await context.cookies(origin);const sessionCookie=cookies.find(c=>c.name==='__Host-brilliantis_session');assert.ok(sessionCookie?.secure && sessionCookie.httpOnly);assert.equal(sessionCookie.sameSite,'Strict');
 assert.equal(cookies.some(c=>c.name==='__Host-brilliantis_oidc'),false);
 assert.equal(await page.evaluate(()=>localStorage.length),0);assert.equal(await page.evaluate(()=>sessionStorage.length),0);
 const users=await page.evaluate(async()=>await(await fetch('/api/v1/access/users')).json());assert.equal(users.items.length,1);assert.equal(users.items[0].email,'leon@example.invalid');
 await mkdir('artifacts',{recursive:true});await page.screenshot({path:'artifacts/oidc-authenticated.png',fullPage:true});
 await page.getByRole('button',{name:'Abmelden',exact:true}).click();
 await page.getByRole('link',{name:'Mit Test-SSO anmelden',exact:true}).waitFor();
 assert.equal(logouts,1);assert.equal((await context.cookies(origin)).some(c=>c.name==='__Host-brilliantis_session'),false);
 assert.equal(await page.evaluate(async()=> (await fetch('/api/v1/dashboard')).status),401);
 for(const name of ['same_email_other_sub','wrong_issuer','wrong_audience','wrong_nonce','expired','future_iat','wrong_azp','missing_azp','missing_nonce','unknown_key','bad_signature','hmac']){
  scenario=name;
  await page.getByRole('link',{name:'Mit Test-SSO anmelden',exact:true}).click();
  await page.getByRole('alert').filter({hasText:'SSO-Anmeldung konnte nicht abgeschlossen'}).waitFor();
  assert.equal(await page.evaluate(async()=> (await fetch('/api/v1/dashboard')).status),401,name);
  assert.equal((await context.cookies(origin)).some(c=>c.name==='__Host-brilliantis_session'),false,name);
 }
 scenario='email_changed';
 await page.getByRole('link',{name:'Mit Test-SSO anmelden',exact:true}).click();
 await page.getByRole('heading',{name:'Gute Ideen. Nächster Schritt.'}).waitFor();
 assert.equal((await page.evaluate(async()=>await(await fetch('/api/v1/auth/session')).json())).user.id,session.user.id,'Identity survives email claim change');
 assert.equal(exchanges,14);assert.deepEqual(providerErrors,[]);assert.deepEqual(pageErrors,[]);
 console.log('OIDC HTTPS browser passed: PKCE/S256, state/nonce, RSA verification, secure server session, admin issuer/sub binding, logout, 12 rejected identity/token attacks, stable identity after email change.');
}finally{
 if(browser)await browser.close();await stop(app);await close(proxy);await close(provider);await rm(temp,{recursive:true,force:true});
}
