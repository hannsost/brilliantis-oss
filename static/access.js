"use strict";
window.BrilliantisAccess = {
  user: null,
  csrf: null,
  demo: false,
  async session() {
    const response = await fetch('/api/v1/auth/session', {credentials:'same-origin'});
    if (!response.ok) throw new Error('Anmeldung konnte nicht geprüft werden.');
    const data = await response.json();
    this.user = data.authenticated ? data.user : null;
    this.csrf = data.csrf || null;
    this.demo = Boolean(data.demo);
    return data;
  },
  async start() {
    const data = await this.session();
    if (data.authenticated) { this.decorate(); return; }
    document.body.classList.add('signed-out');
    const container = document.querySelector('#workspace');
    container.replaceChildren();
    const panel = document.createElement('section');
    panel.className = 'login-panel panel';
    panel.innerHTML = '<img src="/favicon.svg" width="48" height="48" alt=""><p class="eyebrow">BRILLIANTIS</p><h1>Willkommen zurück.</h1><p>Dein Unternehmen. Ein gemeinsamer Arbeitsbereich.</p><form id="sign-in"><label>E-Mail<input name="email" type="email" autocomplete="username" required autofocus></label><label>Passwort<input name="password" type="password" autocomplete="current-password" required maxlength="1024"></label><p class="form-error" role="alert"></p><button type="submit" class="button primary">Anmelden</button></form>';
    container.append(panel);
    const ssoError = new URLSearchParams(location.search).get('sso_error');
    if (ssoError) {
      const hint=document.createElement('p');hint.className='form-error';hint.setAttribute('role','alert');
      hint.textContent=ssoError==='logout_failed'?'Du bist lokal abgemeldet. Die Rückmeldung des Anmeldedienstes konnte nicht bestätigt werden.':'Die SSO-Anmeldung konnte nicht abgeschlossen werden. Erneut anmelden oder die Kontozuordnung prüfen lassen.';
      panel.append(hint);
      history.replaceState(null,'',location.pathname+location.hash);
    }
    if (data.oidc?.enabled) {
      const link=document.createElement('a');link.className='button primary';link.href='/api/v1/auth/oidc/login';link.textContent=data.oidc.label;
      panel.insertBefore(link,panel.querySelector('form'));
      const caption=document.createElement('p');caption.className='muted';caption.textContent='Oder mit deinem lokalen Zugang anmelden.';
      panel.insertBefore(caption,panel.querySelector('form'));
    }
    if (data.setup_required) {
      panel.querySelector('h1').textContent = 'Brilliantis einrichten.';
      panel.querySelector('form').remove();
      const hint = document.createElement('p');
      hint.textContent = 'Für diese Installation muss zuerst ein Administratorkonto eingerichtet werden. Die Schritte stehen in der Installationsanleitung.';
      panel.append(hint);
      return new Promise(()=>{});
    }
    await new Promise(resolve => {
      panel.querySelector('form').addEventListener('submit', async event => {
        event.preventDefault();
        const form = event.currentTarget;
        const button = form.querySelector('button');
        button.disabled = true;
        try {
          const response = await fetch('/api/v1/auth/login', {method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json'},body:JSON.stringify(Object.fromEntries(new FormData(form)))});
          const result = await response.json();
          if (!response.ok) throw new Error(result.error?.message || 'Anmeldung fehlgeschlagen.');
          form.reset();
          await this.session();
          document.body.classList.remove('signed-out');
          this.decorate();
          resolve();
        } catch (error) { form.querySelector('.form-error').textContent = error.message; button.disabled = false; }
      });
    });
  },
  decorate() {
    const existing = document.querySelector('#session-actions');
    if (existing) existing.remove();
    if (this.demo) return;
    const actions = document.createElement('div');
    actions.id = 'session-actions';
    const name = document.createElement('span');
    name.className = 'muted';
    name.textContent = this.user.name;
    const button = document.createElement('button');
    button.className = 'text-button';
    button.textContent = 'Abmelden';
    button.addEventListener('click', async () => {
      button.disabled=true;
      try {
        const response=await fetch('/api/v1/auth/logout', {method:'POST',credentials:'same-origin',headers:{'X-CSRF-Token':this.csrf}});
        if(!response.ok) throw new Error('Abmeldung fehlgeschlagen.');
        const result=await response.json();
        if(result.redirect_url) location.assign(result.redirect_url); else location.reload();
      } catch(error) { button.disabled=false; button.textContent='Abmelden erneut versuchen'; }
    });
    actions.append(name, button);
    document.querySelector('.topbar-right').prepend(actions);
  }
};
