# Brilliantis

**Agent-first CRM und ERP in Rust · MIT · Version 0.2.0**

Brilliantis verbindet Vertrieb, Geschäftspartner, Angebote, Verträge, Rechnungen, Kontoumsätze und Kampagnen in einem gemeinsamen Arbeitsbereich. Menschen nutzen die eingebettete Oberfläche; Agenten verwenden dieselben Fachaktionen über REST/OpenAPI.

| Distribution | Inhalt |
| --- | --- |
| **Standalone** | Eigenständig startbar, eigener Produktkatalog, lokale Benutzer und optionale Anbieteranschlüsse. Kein NeuroFunken-Konto erforderlich. |
| **Für NeuroFunken** | Eigenes Paket mit Branding, Produktvorlagen, Website-Zuordnung und Konfigurationsbeispielen für unsere Systemintegration. |

Ein Cargo-Workspace, ein gemeinsamer MIT-Kern und zwei Binaries. Der Kern hängt nicht vom NeuroFunken-Paket ab. Self-Hosting, betreute Kundeninstallation und eigenes SaaS sind unabhängige Betriebsformen. Die geplanten Leistungen umfassen Einrichtung, Migration, Adapterkonfiguration, Agentenbetrieb, Updates, Sicherungen und Support. Die NeuroFunken-Installation ist unter https://brilliantis.neurofunken.de erreichbar und verlangt eine Anmeldung. Neue lokale Checkpoints sind erst nach einem gesondert geprüften Betreiber-Rollout dort verfügbar.

## Lokal ansehen

```sh
cargo build --locked --release --workspace
# NeuroFunken-Vorschau, eigene Datenbank
./target/release/brilliantis-neurofunken --demo --port 4319 --database data/workspace-preview.sqlite
# http://127.0.0.1:4319

# Unabhängige Standalone-Demo
./target/release/brilliantis --demo --port 4320
# http://127.0.0.1:4320
```

Die Demo ist ohne Anmeldung nur über Loopback erreichbar. Firmen, Beträge und Profile sind fiktiv; externe Ausführung ist ausgeschaltet. Änderungen bleiben über Neustarts erhalten. Unternehmensdaten, ausgegebene Rechnungen, zugeordnete Zahlungen und Bankgebühren zeigen den Zusammenhang zwischen Journal und Kennzahlen. Dashboard und Finanzbericht unterscheiden geschätztes Dealvolumen, gebuchten Umsatz, offene Posten und Liquidität.

Rust und ein C-Compiler genügen zum Bauen. Zur Laufzeit werden weder Node noch ein separater Datenbankserver benötigt. HTML, CSS, JavaScript und SVG-Logo sind im Binary enthalten; keine CDN-Abhängigkeit. SQLite läuft mit WAL und einem Schreiber. Ein belegter Port wird nicht übernommen. `--port 0` wählt einen freien Port, `--help` zeigt die Optionen.

## Eigene Arbeitsinstanz

Ein Passwort mit mindestens zwölf Zeichen in einer privaten Datei bereitstellen; die Datei nicht ins Repository aufnehmen:

```sh
./target/release/brilliantis --database data/company.sqlite \
  --init-admin admin@example.org --password-file /geschuetzter/pfad/admin-password
./target/release/brilliantis --database data/company.sqlite --port 4321
```

Anschließend unter `http://127.0.0.1:4321` anmelden und die Unternehmensdaten einrichten. Benutzerrollen sind Administration, Vertrieb, Buchhaltung und lesender Zugriff. Agentenzugänge werden in „Team & Zugänge“ erstellt, einmalig angezeigt und können eingeschränkt und widerrufen werden. Passwörter sind mit Argon2 gehasht; Sitzungs- und API-Token werden nur gehasht gespeichert. Cookie-Zugriffe verwenden CSRF-Prüfung, API-Zugriffe Bearer-Token. Die verifizierte Identität bestimmt den Urheber einer Änderung.

Eine Datenbank ist an ihre Distribution und Demo-/Arbeitsmodus gebunden. Für jede Organisation wird eine eigene Installation mit eigener Datenbank, Secrets und Zugangsverwaltung betrieben. Gemeinsame Mandantentabellen und mehrere SQLite-Schreibreplicas sind nicht implementiert.

## Umgesetzte Abläufe

| Bereich | Verfügbar |
| --- | --- |
| Vertrieb | Leads und Pipeline, Geschäftspartner, Produktpflege, Angebote mit Positionen, Ausgabe mit Stammdatensnapshot, Annahme mit einmaliger Vertragserzeugung. |
| Rechnungen | Ein- und Ausgangsrechnungen in EUR, Nummernkreise, unveränderliche ausgegebene Belege, vollständige Stornierung und Druckansichten. |
| Buchungen | Ausgeglichenes Journal, Kontoumsätze mit Quellen-Deduplizierung, Zahlungszuordnung, Gebühren/Sachkonten und Erstattungszuordnung. Beträge werden in Ganzzahlen berechnet. |
| Kennzahlen | Umsatz, Aufwand, Ergebnis, Liquidität, Umsatzsteuer-/Vorsteuerkonten, Monatsverlauf, Salden und aktuelle offene/überfällige Posten. Konfigurierbare Schwellwerte und Hinweise auf ungeklärte Umsätze, Entwürfe und Erstattungen. |
| Abschlussvorbereitung | Jahresauswahl, Anfangs-/Perioden-/Endsalden, Monatsabschluss mit Buchungssperre und begründetem Wiederöffnen, Journal-CSV. |
| Marketing | Kampagnen, eigene Marken-/virtuelle Profile und externe Influencer, Kanalaccounts, Contentplanung, Kooperationen mit Budget/Rechten, SEO-Themen und Shopbestellungen. |
| Plattform | Anmeldung, Rollen, Agententoken, versionierte Fachaktionen, Idempotenzbelege, Ereigniscursor, durchsuchbare paginierte Datensätze, Archivierung unterstützter Stammdaten. |
| Website / Portal | Versionierter Commerce-Vertrag: externer Kunde, Katalogrevision, serverseitige Kalkulation, Projektangebot, Annahme mit genau einem Auftrag/Vertrag, Bereitstellungsstatus, Erstrechnung und buchungsbasierter Zahlungsstatus. |
| Betrieb | Versionierte geprüfte Migrationen, konsistente Sicherung/Wiederherstellung, dauerhafte Integrationsaufträge, Helm und Containerdefinition. |

Die Finanzberichte sind **Abschlussvorbereitung**, keine fertige handelsrechtliche Bilanz oder Steuererklärung. Der CSV-Export ist ein Journalexport, kein DATEV-EXTF. Noch fehlen unter anderem Kontenrahmen-Zuordnung, Anlagen-/Lohnbuchhaltung, steuerliche Sonderfälle, Fremdwährungen, Teilgutschriften, wiederkehrende Fakturierung und Steuerübermittlung. Historische Journalwerte und **aktuelle** offene Posten sind im Bericht getrennt bezeichnet.

Listen unter `/api/v1/records/…` werden serverseitig durchsucht und paginiert. Die älteren Pipeline-/Kampagnen-/Influencer-Übersichten zeigen bis zu 200 Datensätze; Kennzahlen werden über den gesamten aktiven Bestand berechnet. Die umfangreiche Stammdatenbearbeitung der drei älteren Bereiche ist derzeit über die API erreichbar.

## Briefings und manuelle Übergabe

„Briefings & Übergaben“ verwaltet dauerhafte Grundbriefings und kurze Ergänzungen in beiden Distributionen. Empfänger, Sprache, Kanäle, Kampagne und Influencer-Profil werden am Entwurf gepflegt. Eine Ergänzung verweist auf eine konkrete bereitgestellte Fassung ihres Grundbriefings. Das Anlegen registriert kein Konto und lädt keine distributionsspezifischen Persona-Dateien automatisch.

„Fassung bereitstellen“ speichert einen unveränderlichen TXT-Snapshot mit SHA256. Spätere Bearbeitungen verändern nur den Entwurf; ältere Fassungen bleiben herunterladbar, auch im Archiv. Erst „Übergabe erfassen“ dokumentiert die tatsächliche manuelle Weitergabe mit Urheber, Zeitpunkt und Notiz. Das ist kein Nachweis für einen laufenden Bot. Sales kann Grundtexte und kurze Ergänzungen liefern, ohne Einzelposts freizugeben oder selbst zu veröffentlichen.

Agenten verwenden `/api/v1/briefings` mit `crm:read`/`crm:write` über die vorhandenen Benutzer-/OIDC-Zugänge. Die enge Website-Maschinenidentität erhält dadurch **keinen** Zugriff. Änderungen benötigen `Idempotency-Key` und die aktuelle `version`; Wiederholungen behalten Body und Schlüssel bei. Aktionen: `release`, `handoff` (zusätzlich `revision` und `note`), `archive`, `restore`. `/api/v1/briefings/{id}/editions` liefert die Fassungshistorie; `/api/v1/briefings/{id}/editions/{revision}/download` liefert das TXT ohne Schreibwirkung. OpenAPI beschreibt die Felder. Ereignisse heißen `briefing.create/edit/release/handoff/archive/restore`.

Migration 008 ergänzt ausschließlich Briefingtabellen. Vor dem Rollout sichern; ein älteres Binary akzeptiert Schema 008 nicht. Ein Rückwechsel erfordert die Sicherung vor der Migration. Die vorhandenen externen Grokbot-Startpakete bleiben separat; eine automatische Auslieferung oder Aktivierung wird hier nicht durchgeführt.

## Website, Hosting und SaaS

[Integrationsvertrag v1](commerce-v1.json) und OpenAPI beschreiben die konkreten Felder, Scopes und Ereignisse. Das Portal verwendet einen Backend-Zugang mit `commerce:read,commerce:write`; Bereitstellung erhält `commerce:fulfill`, Rechnungserstellung `finance:write`. Diese Rechte gelten für den gesamten Unternehmensarbeitsbereich. Das Kundenportal prüft die Zuordnung seiner Endnutzer selbst.

Der Betreiber veröffentlicht unveränderliche Katalogrevisionen. Brilliantis berechnet Hosting-/SaaS-Angebote aus diesen Preisen und validierten Konfigurationsdimensionen. Website-, Webapp- und andere Projektleistungen benötigen erst ein individuelles ausgegebenes Angebot. Annahme, Vertrag, Auftrag und Wiederholungsbeleg werden gemeinsam gespeichert; bei einem Timeout denselben Body und Idempotency-Key erneut senden. Alternativ lässt sich der Auftrag über seine externe Referenz wiederfinden. Die Oberfläche zeigt Portalaufträge unter „Aufträge & Shops“ mit Abrechnung und Bereitstellungsstatus.

Wiederkehrende Positionen behalten ihren Rhythmus `monthly` oder `yearly`; die Erstrechnung enthält Einmalleistungen und den ersten Zeitraum. Folgeabrechnung, Tarifwechsel, automatische Steuergebietsbestimmung und Kündigungs-/Abbauorchestrierung sind noch nicht automatisiert. Ein Bereitstellungsstatus ist eine Meldung des authentifizierten Workers. Zahlungsstatus stammt ausschließlich aus Rechnung und Buchungszuordnung. Der Vertrag ersetzt den bisherigen unverbindlichen Website-Anfragenimport nicht; beide haben getrennte Zwecke.

## Anbieteranschlüsse

Ausführbare HTTP-Adapter sind vorhanden für Mollie, Stripe, PayPal, Enable Banking, Shopware, WooCommerce, Shopify, Gmail, Search Console, Instagram, TikTok, Discourse, SAP Business One, SAP S/4HANA, DATEV Accounting und Storecove/Peppol sowie SolveIT. Die unterstützten Operationen liefert `/api/v1/connections`; sie sind ausdrücklich pro Anschluss freizuschalten.

```sh
./target/release/brilliantis --database data/company.sqlite \
  --connectors-file /konfiguration/connectors.json
```

[Allgemeines Beispiel](../config/connectors.example.json) und [NeuroFunken-Beispiel](../distributions/neurofunken/connectors.example.json) enthalten Platzhalter und Secret-Referenzen. Kundenspezifische API-Hosts, IDs, Versionen, erlaubte Operationen und Provider-Zugänge müssen konfiguriert werden. Das NeuroFunken-Profil allein aktiviert keine Verbindung. Die Adapter wurden anhand der Anbieter-Schnittstellen implementiert und mit lokalen Fixtures geprüft; ein erfolgreicher Live-Abgleich mit den Anbietern ist nicht nachgewiesen.

Integrationsaufträge werden vor Netzverkehr gespeichert. Wiederholungen verwenden eine stabile Auftragskennung. Nach unklarer Zustellung werden nicht sicher wiederholbare Schreiboperationen als `uncertain` markiert. Der Worker prüft lokale Benutzerrechte und den Widerruf eigener Agententoken erneut. Ergebnisse werden von bekannten Secret-Feldern bereinigt. Ein Adapteraufruf ist noch keine automatische Synchronisierung: Mapping, Webhook-Verarbeitung, vollständige Seitennachverfolgung, Rückabgleich und Bereitstellung von Hosting-/SaaS-Produkten sind weiter auszubauen.

Social- und E-Mail-Adapter verwenden bereitgestellte offizielle Zugänge. Die Verwaltung legt keine externen Accounts automatisch an. Content als „veröffentlicht“ zu erfassen benötigt eine Veröffentlichungs-URL; das löst keinen Plattform-Post aus. Ein tatsächlicher Versand erfolgt ausschließlich durch eine konfigurierte Schreiboperation.

## Secrets und Keycloak

Secret-Referenzen verweisen entweder auf gemountete Dateien oder auf ein explizites Bitwarden-Secrets-Manager-Projekt und Secret. Beim BWS-Provider werden Server und Machine-Token-Datei vom Betreiber angegeben; `bws` muss im ausführbaren Suchpfad verfügbar sein. Der aufgelöste Wert wird für den jeweiligen Aufruf verwendet, nicht in Fachobjekte übernommen. Die BWS-CLI ist optional und nicht im Standardimage enthalten. Das Beispiel weist keinen vorhandenen Zugang zu.

Keycloak-Bearer-Token können über RFC-7662-Introspection geprüft werden:

```sh
./target/release/brilliantis --database data/company.sqlite \
  --oidc-file /konfiguration/oidc.json
```

[OIDC-Konfiguration](../config/oidc.example.json) und [NeuroFunken-Beispiel](../distributions/neurofunken/oidc.example.json) unterstützen zusätzlich native Browser-Anmeldung mit `browser_login: true`. Keycloak verwendet einen vertraulichen Client mit Authorization Code, PKCE/S256 und RS256-signierten ID-Tokens. `state` ist einmalig und über ein HttpOnly/SameSite=Lax-Cookie an den Browser gebunden; `nonce`, Signatur, Issuer, Audience, Zeitangaben und `azp` werden geprüft. Der Browser erhält ausschließlich eine neue zufällige Sitzung, keine Provider-Tokens. Die Sitzung liegt gehasht in SQLite, verwendet im HTTPS-Betrieb ein Secure/HttpOnly/SameSite=Strict-Cookie und läuft nach 30 Minuten Inaktivität beziehungsweise acht Stunden absolut ab. Lokale Rollen und Sperren werden bei jedem Zugriff erneut geprüft.

Callback: `{public-url}/api/v1/auth/oidc/callback`. Rückkehr nach Logout: `{public-url}/api/v1/auth/oidc/logout/callback`. Beide URLs werden exakt aus dem konfigurierten Origin erzeugt; keine Wildcards registrieren. Anmeldestart: `GET /api/v1/auth/oidc/login`; angeforderte Scopes: `openid profile email`. `POST /api/v1/auth/logout` mit Origin und CSRF löscht zuerst die lokale Sitzung und liefert bei SSO eine Keycloak-Logout-URL. Ohne gespeicherten ID-Token-Hinweis kann Keycloak dabei eine Abmeldebestätigung zeigen. Provider-Sitzungen werden nicht regelmäßig erneuert; Backchannel-Logout anderer Anwendungen ist in dieser Version nicht implementiert. Die lokale Sitzungsdauer ist unabhängig von der kurzen ID-Token-Laufzeit.

Konten werden **explizit über Issuer und Subject** verknüpft. Eine E-Mail aus einem Token legt kein Konto an, verknüpft kein vorhandenes Konto und vergibt keine Rolle. Der Betreiber kann `bootstrap_admin: {"subject":"<verifizierter Keycloak-sub>","email":"admin@example.org","name":"Admin"}` in die OIDC-Konfiguration ergänzen. Diese vertrauenswürdige Konfiguration legt das Administratorkonto an oder verknüpft ein bestehendes, noch unverknüpftes aktives Konto. Folgende Starts erhalten spätere Rollenänderungen und Sperren. Neue SSO-Konten besitzen dabei kein lokales Passwort. Die Zuordnung kann administrativ und versioniert über `PUT /api/v1/access/users/{id}/oidc` mit `version`, `issuer`, `subject` geändert werden; bestehende Sitzungen und API-Token des Kontos werden dann widerrufen. Alternativ bindet `POST /api/v1/access/users` mit `oidc_subject` neue Konten an den ausdrücklich konfigurierten Issuer.

Migration 006 übernimmt alte Subject-Einträge ohne bekannten Issuer bewusst nicht automatisch. Bestehende Zuordnungen müssen ausdrücklich verifiziert und gebunden werden. Für private Unternehmens-CAs kann `ca_certificate_file` angegeben werden; die TLS-Prüfung bleibt aktiv. Der lokale HTTPS-Testanbieter in `scripts/browser-oidc.mjs` prüft den vollständigen Browserablauf mit temporären Schlüsseln sowie zurückgewiesenen Identitäts- und Tokenmanipulationen. Die Umsetzung folgt den [Keycloak-Endpunkten](https://www.keycloak.org/securing-apps/oidc-layers), [OIDC-Tokenprüfungen](https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation), [PKCE](https://www.rfc-editor.org/rfc/rfc7636) und [RP-Initiated Logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html).

## E-Rechnungen

Ausgegebene Ausgangsrechnungen können UBL-XML für XRechnung 3.0 und CII-XML für EN 16931 erzeugen. Die Downloads sind ausdrücklich **unvalidiert** gekennzeichnet. Pflichtangaben wie Käuferreferenz, Kontakt und Bankverbindung werden vor Ausgabe geprüft; eine vollständige Schema-/Schematronprüfung gehört vor den Versand.

`scripts/zugferd.sh` beschreibt den separaten Mustang-CLI-Worker: vorhandenes passendes PDF/A, CII-Datei und Mustang-JAR angeben; XML validieren, PDF/A-3 zusammensetzen, Ergebnis validieren. Die Ausgabe wird nur nach erfolgreichem Prüflauf veröffentlicht. Java/Mustang sind optionale Werkzeuge und nicht im Rust-Server enthalten. Dieser Worker ist lokal noch nicht mit einem realen Validatorlauf nachgewiesen. XML allein ist kein ZUGFeRD-PDF. Peppol-Transport erfolgt über den konfigurierten Access-Point-Adapter, nicht durch einen eigenen zertifizierten Access Point.

## Agenten-API

Vertrag: `/api/v1/openapi.json` · Kontext: `/api/v1/agent/context` · Ereignisse: `/api/v1/events?after=0`.

```sh
# Lokal in der Demo; in einer Arbeitsinstanz zusätzlich Authorization: Bearer …
curl http://127.0.0.1:4319/api/v1/agent/context
curl 'http://127.0.0.1:4319/api/v1/records/customers?limit=30&q=Studio'
curl 'http://127.0.0.1:4319/api/v1/finance/report?year=2026'
```

Fachliche Schreiboperationen benötigen `Idempotency-Key`; bei Retry Body und Key beibehalten. Updates brauchen die zuletzt gelesene `version`; bei `409` neu lesen. Ein fehlgeschlagener Fachbefehl schreibt keine Teilbuchung. Listen liefern `items`, `next_cursor` und `has_more`. Events liefern bis zu 100 Einträge. Request-Schemas werden aus Rust-Typen erzeugt. `X-Brilliantis-Client` ist nur in der lokalen Demo ein frei wählbares Label. REST ist implementiert; ein MCP-Server und Symphony-Claims sind es noch nicht.

## Maschinenzugänge

Operatorseitige Maschinenzugänge für Website und Portal verwenden eigene Identitäten ohne Benutzerkonto, Passwort oder SSO-Sitzung. Der Weg steht in beiden Distributionen zur Verfügung. Zuerst die Arbeitsinstanz mit diesem Binary auf das aktuelle Datenbankschema aktualisieren; die CLI selbst migriert keine Datenbank und akzeptiert weder Demo noch abweichendes Profil. Sie kann anschließend auf derselben lokalen Datenbank neben dem Server laufen; Schreiboperationen werden transaktional serialisiert.

```sh
mkdir -m 0700 /data/operator-tokens
brilliantis machine provision --database /data/brilliantis.sqlite \
  --name neurofunken-website --generation inquiry-v1 --scopes crm:write \
  --ttl-seconds 2592000 --output /data/operator-tokens/inquiry-v1.json
```

Die Ausgabe ist eine neue, ausschließlich dem ausführenden Unix-Benutzer gehörende 0600-JSON-Datei mit `schema_version`, `machine`, `generation`, `token`, `scopes`, `created_at` und `expires_at` (Unix-Sekunden). Den Wert `token` geschützt in die Secret-Verwaltung des Website-Backends übernehmen und als Bearer senden. Kein Token erscheint in stdout/stderr oder Logs; die Datenbank speichert nur seinen Hash. Absolute Pfade und ein eigenes privates Zielverzeichnis sind erforderlich. Bestehende Dateien, Links, fremde Eigentümer und unsichere Verzeichnisrechte werden abgelehnt. Auch die Container-UID 10001 benötigt ein eigenes beschreibbares Verzeichnis mit Modus 0700. Es gibt keine stdout-Ausgabeoption für Secrets.

Erlaubte Scopes sind ausschließlich `crm:write`, `commerce:read` und `commerce:write`, jeweils explizit kommagetrennt. Es gibt keine voreingestellten Scopes; die Laufzeit muss ausdrücklich zwischen 60 Sekunden und 90 Tagen liegen. Maschinen dürfen nur `/api/v1/import/website` und `/api/v1/commerce/…` entsprechend ihrer Scopes nutzen. Administration, Finanzzugriffe, Benutzer-Token und Provider-Jobs sind ausgeschlossen. Das sind Zugänge für ein gesamtes Workspace; die Website muss die Berechtigung ihrer Endkunden selbst prüfen.

Ein identischer Aufruf mit derselben Generation, Laufzeit, Scopes und Ausgabe bestätigt ausschließlich den bestehenden, weiterhin gültigen Zugang. Die Datei wird auf Rechte und Inhalt geprüft und nicht erneut geschrieben. Geänderte Parameter, fehlende Ausgaben, abgelaufene oder widerrufene Generationen führen zum Fehler. Eine neue Generation erfordert ausdrücklich `renew` mit der aktuellen Ausgangsgeneration:

```sh
brilliantis machine renew --database /data/brilliantis.sqlite \
  --name neurofunken-website --from-generation inquiry-v1 --generation inquiry-v2 \
  --scopes crm:write --ttl-seconds 2592000 --output /data/operator-tokens/inquiry-v2.json
brilliantis machine revoke --database /data/brilliantis.sqlite \
  --name neurofunken-website --generation inquiry-v2
```

Erneuerung aktiviert den neuen Zugang und widerruft den alten in derselben Transaktion; es gibt keine Überlappung. Den Website-Transport für die Secret-Übernahme kurz pausieren bzw. seine Warteschlange nutzen. `machine:<name>` bleibt als Akteur stabil: Nach Erneuerung bleiben Anfragen mit ursprünglichem Body und Idempotency-Key wiederholbar. Widerruf ist wiederholbar und betrifft nur die genannte Generation. Eine ausdrücklich erneuerte Generation kann auch einen abgelaufenen oder widerrufenen Zugang ersetzen. Restore widerruft sämtliche Maschinen-Token.

Datei und Datenbank können nicht in einer gemeinsamen Transaktion gespeichert werden: Die Ausgabe wird vor dem Datenbank-Commit synchronisiert. Bei einem Abbruch kann eine private Datei ohne gültigen Datenbankeintrag verbleiben. Sie wird nie automatisch überschrieben oder importiert. Nach Prüfung kann der Betreiber bei fehlendem Eintrag mit einem neuen Dateipfad erneut provisionieren; bei bereits gespeichertem, aber verlorenen oder beschädigten Token ausdrücklich mit neuer Generation erneuern. Fehler während einer Erneuerung vor dem Commit erhalten den alten Zugang. Zugangswerte niemals zur Diagnose ausgeben.

## Sicherung und Wiederherstellung

```sh
# Konsistenter SQLite-Snapshot, auch bei laufender Anwendung; Ziel muss neu sein.
./target/release/brilliantis --database data/company.sqlite --backup /sicherung/company-20261007.sqlite
# Quelle bleibt erhalten; neue Zieldatenbank, passendes Distributionsbinary verwenden.
./target/release/brilliantis --database data/restored.sqlite --restore /sicherung/company-20261007.sqlite
```

Integritätsprüfung, Fremdschlüssel und Migrationsprüfsummen werden kontrolliert. Restore widerruft Sitzungen/API-Token und markiert offene Integrationsaufträge zur Prüfung. Bestehende Ziele werden nicht überschrieben. Wiederhergestellte Integrationsaufträge nicht ungeprüft wiederholen; extern bereits ausgeführte Aktionen werden durch einen Datenbank-Restore nicht rückgängig. Automatische Sicherungsplanung, externes Backupziel und Aufbewahrung sind Betreiberaufgaben. Downgrade ist nicht implementiert.

## Container und Helm

```sh
podman build -t brilliantis:0.2.0 .
podman build --build-arg PACKAGE=brilliantis-neurofunken -t brilliantis-neurofunken:0.2.0 .
helm lint deploy/helm/brilliantis
helm template brilliantis deploy/helm/brilliantis
helm template brilliantis deploy/helm/brilliantis -f distributions/neurofunken/values.yaml
```

Der [Chart](../deploy/helm/brilliantis/values.yaml) verwendet eine SQLite-Replica, PVC, Recreate, einen nicht privilegierten Container, Read-only-Root und Healthchecks. Build- und Distroless-Laufzeitimage sind per Digest festgelegt; das Runtimeimage enthält keine Shell. Standardmäßig ist er per Port-Forward erreichbar. Für Netzwerkbetrieb `network.enabled=true` und einen HTTPS-Origin in `network.publicUrl` setzen. Optionaler Ingress benötigt ein TLS-Secret. Bootstrap verwendet ein bestehendes Secret, Connector-/OIDC-Konfigurationen bestehende ConfigMaps und getrennte Secret-Mounts. Optional kann eine NetworkPolicy gesetzt werden.

TLS endet am Reverse Proxy/Ingress; der Rust-Prozess spricht intern HTTP. Der konfigurierte Host und Origin müssen erhalten bleiben. Das zur Distribution passende Image muss selbst gebaut und im Zielcluster verfügbar sein. Ein Registry-Push, Cluster-Rollout und produktiver SaaS-Betrieb sind nicht erfolgt. Vor Upgrades Daten sichern; der PVC bleibt bei Deinstallation erhalten.

## Entwickeln und prüfen

```sh
cargo fmt --all --check
cargo test --locked --workspace -j 2
cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings
cargo build --locked --release --workspace -j 2
```

Die Tests prüfen unter anderem Idempotenz, Versionskonflikte, Rechte/CSRF, Rechnungs-/Zahlungs-/Stornobuchungen, Periodensperren, Migrationen, Backup/Restore, Adapterpläne und lokalen HTTP-Transport. XML-Parsingtests ersetzen keine fachliche E-Rechnungsvalidierung. Der HTTP-Test benötigt Loopback-Sockets.

```sh
npm ci --ignore-scripts
BRILLIANTIS_TEST_BINARY=target/release/brilliantis npm run test:browser
BRILLIANTIS_TEST_BINARY=target/release/brilliantis npm run test:access
BRILLIANTIS_TEST_PROFILE=neurofunken BRILLIANTIS_TEST_BINARY=target/release/brilliantis-neurofunken npm run test:browser
```

Die Browserprüfung nutzt temporäre Datenbanken und Chromium, prüft Geschäftsabläufe und Desktop-/Mobilansichten und schreibt Screenshots nach `artifacts/`. `BRILLIANTIS_CHROMIUM` und `BRILLIANTIS_PLAYWRIGHT_MODULE` überschreiben lokale Werkzeugpfade. Diese Entwicklungswerkzeuge sind keine Laufzeitabhängigkeit der Anwendung.

## Lizenz

[MIT](../LICENSE). Abhängigkeiten und optionale Werkzeuge behalten ihre jeweiligen Lizenzen. Anbieterzugänge zu DATEV, SAP, Zahlungsdiensten und Plattformen sind nicht Bestandteil der MIT-Lizenz von Brilliantis.
