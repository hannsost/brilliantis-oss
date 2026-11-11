#!/usr/bin/env python3
"""Exercise a locally built image with temporary workspace data, without external APIs."""
import json
import os
from pathlib import Path
import secrets
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

image = os.environ.get('BRILLIANTIS_TEST_IMAGE', 'localhost/brilliantis-neurofunken:0.2.0-rc')
container = None

def podman(*args, check=True):
    return subprocess.run(['podman', *args], text=True, capture_output=True, check=check)

with tempfile.TemporaryDirectory(prefix='brilliantis-container-') as directory:
    root = Path(directory)
    data = root / 'data'
    data.mkdir()
    data.chmod(0o700)
    password = secrets.token_urlsafe(24)
    password_file = root / 'bootstrap-password'
    password_file.write_text(password)
    password_file.chmod(0o600)
    common = ['--read-only', '--cap-drop=ALL', '--security-opt=no-new-privileges',
              '--userns=keep-id:uid=10001,gid=10001', '--tmpfs=/tmp:rw,nosuid,nodev,size=32m',
              '-v', f'{data}:/data:rw']
    try:
        podman('run', '--rm', '--network=none', *common, '-v', f'{password_file}:/bootstrap/password:ro',
               image, '--database', '/data/workspace.sqlite', '--init-admin', 'admin@example.invalid',
               '--password-file', '/bootstrap/password', '--init-if-empty')
        container = podman('run', '-d', *common, '-p', '127.0.0.1::4317', image,
                           '--database', '/data/workspace.sqlite', '--bind', '0.0.0.0',
                           '--public-url', 'https://sales.example.invalid').stdout.strip()
        address = podman('port', container, '4317').stdout.strip()
        origin = f'http://{address}'

        def request(path, method='GET', body=None, extra=None):
            headers = {'Host': 'sales.example.invalid', **(extra or {})}
            if body is not None:
                headers['Content-Type'] = 'application/json'
                body = json.dumps(body).encode()
            req = urllib.request.Request(origin + path, data=body, method=method, headers=headers)
            # Never follow a login or proxy redirect to an external location.
            class NoRedirect(urllib.request.HTTPRedirectHandler):
                def redirect_request(self, req, fp, code, msg, headers, newurl):
                    return None
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
            try:
                response = opener.open(req, timeout=3)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                return response.status, response.headers, response.read()

        for attempt in range(100):
            try:
                if request('/readyz')[0] == 200:
                    break
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.1)
        else:
            raise AssertionError('Container readiness timeout')
        podman('exec', container, '/usr/local/bin/brilliantis', '--healthcheck')
        for path in ['/api/v1/dashboard', '/api/v1/finance/report', '/api/v1/records/invoices']:
            assert request(path)[0] == 401, f'Anonymous access to {path}'
        assert request('/api/v1/dashboard', extra={'Host': 'foreign.example.invalid'})[0] == 403
        status, headers, raw = request('/api/v1/auth/login', 'POST',
            {'email': 'admin@example.invalid', 'password': password}, {'Origin': 'https://sales.example.invalid'})
        assert status == 200, 'Container login failed'
        session = json.loads(raw)
        cookie = headers['Set-Cookie'].split(';')[0]
        assert cookie.startswith('__Host-brilliantis_session=') and '; Secure' in headers['Set-Cookie']
        auth = {'Cookie': cookie, 'Origin': 'https://sales.example.invalid', 'X-CSRF-Token': session['csrf']}
        status, _, raw = request('/api/v1/access/tokens', 'POST',
            {'name':'website-fixture','scopes':['crm:read','crm:write'],'expires_in_days':1}, auth)
        assert status == 200, 'Scoped token creation failed'
        bearer = {'Authorization': 'Bearer ' + json.loads(raw)['token'], 'Idempotency-Key':'container-inquiry-1'}
        inquiry = {'id':'container-inquiry-1','created_at':'2026-10-07T12:00:00Z','name':'Fixture',
                   'email':'fixture@example.invalid','company':'Fixture','topic':'hosting','message':'Hosting inquiry',
                   'consent':True,'hosting':{'schema_version':1,'quote':{'monthlyCents':10000}}}
        first = request('/api/v1/import/website', 'POST', inquiry, bearer)
        assert first[0] == 200, 'Authenticated intake failed'
        assert json.loads(first[2])['record']['amount_cents'] == 0
        assert request('/api/v1/import/website','POST',inquiry,bearer)[2] == first[2], 'Intake retry changed response'
        assert request('/api/v1/finance/report', extra=bearer)[0] == 403, 'Intake token can read finance'
        machine = ['machine', 'provision', '--database', '/data/workspace.sqlite',
                   '--name', 'website', '--generation', 'initial', '--scopes', 'crm:write',
                   '--ttl-seconds', '3600', '--output', '/data/website-initial.json']
        issued = podman('exec', container, '/usr/local/bin/brilliantis', *machine)
        machine_file = data / 'website-initial.json'
        original_bytes = machine_file.read_bytes()
        credential = json.loads(original_bytes)
        assert credential['token'] not in issued.stdout + issued.stderr
        assert machine_file.stat().st_mode & 0o777 == 0o600
        podman('exec', container, '/usr/local/bin/brilliantis', *machine)
        assert machine_file.read_bytes() == original_bytes, 'CLI retry rotated token'
        machine_auth = {'Authorization': 'Bearer ' + credential['token'], 'Idempotency-Key': 'machine-container-inquiry'}
        machine_inquiry = {**inquiry, 'id': 'machine-container-inquiry'}
        accepted = request('/api/v1/import/website', 'POST', machine_inquiry, machine_auth)
        assert accepted[0] == 200, 'Machine intake failed'
        assert request('/api/v1/access/users', extra=machine_auth)[0] == 403
        renewed = ['machine', 'renew', '--database', '/data/workspace.sqlite',
                   '--name', 'website', '--generation', 'next', '--from-generation', 'initial',
                   '--scopes', 'crm:write', '--ttl-seconds', '3600', '--output', '/data/website-next.json']
        podman('exec', container, '/usr/local/bin/brilliantis', *renewed)
        assert request('/api/v1/import/website', 'POST', machine_inquiry, machine_auth)[0] == 401
        next_credential = json.loads((data / 'website-next.json').read_bytes())
        machine_auth['Authorization'] = 'Bearer ' + next_credential['token']
        assert request('/api/v1/import/website', 'POST', machine_inquiry, machine_auth)[2] == accepted[2], 'Renewal lost idempotency'
        revoke = ['machine', 'revoke', '--database', '/data/workspace.sqlite', '--name', 'website', '--generation', 'next']
        podman('exec', container, '/usr/local/bin/brilliantis', *revoke)
        podman('exec', container, '/usr/local/bin/brilliantis', *revoke)
        assert request('/api/v1/import/website', 'POST', machine_inquiry, machine_auth)[0] == 401
        podman('exec', container, '/usr/local/bin/brilliantis', '--database','/data/workspace.sqlite',
               '--backup','/data/backup.sqlite')
        podman('run','--rm','--network=none',*common,image,'--database','/data/restored.sqlite',
               '--restore','/data/backup.sqlite')
        assert (data/'restored.sqlite').stat().st_size > 0
        with sqlite3.connect(data / 'restored.sqlite') as restored:
            assert restored.execute('SELECT COUNT(*) FROM machine_tokens WHERE revoked=0').fetchone()[0] == 0
        rejected = podman('run','--rm','--network=none',*common,image,'--demo','--bind','0.0.0.0',
                          '--public-url','https://sales.example.invalid',check=False)
        assert rejected.returncode != 0, 'Public demo was accepted'
        print('Container checks passed: non-root/read-only, bootstrap, probes, HTTPS-origin auth, machine CLI/private output/retry/renew/revoke, website replay across renewal, backup/restore, public-demo rejection.')
    finally:
        if container:
            podman('stop','--time','15',container,check=False)
            podman('rm',container,check=False)
