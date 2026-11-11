<p align="center">
  <img src="docs/logo.svg" alt="Brilliantis" height="96">
</p>

<h1 align="center">Brilliantis</h1>

<p align="center"><strong>Agent-first open-source CRM & ERP in a single Rust binary.</strong></p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-0B1A3A.svg"></a>
  <img alt="Stack" src="https://img.shields.io/badge/stack-Rust%20%C2%B7%20SQLite-FF8A1F.svg">
  <a href="CONTRIBUTING.md"><img alt="PRs welcome" src="https://img.shields.io/badge/PRs-welcome-FFC247.svg"></a>
</p>

<p align="center">
  <a href="https://neurofunken.de"><img src="docs/neurofunken-logo.svg" alt="neurofunken" height="28"></a><br>
  <sub>a neurofunken project</sub>
</p>

---

Brilliantis connects sales, business partners, quotes, contracts, invoices, bank transactions and campaigns in one shared workspace. Humans use the embedded web UI; AI agents use exactly the same business actions through a REST/OpenAPI interface — with roles, audit trail and idempotent commands. Brilliantis ships as one self-contained binary with an embedded UI and SQLite, so it runs on a laptop, a small VM or Kubernetes (Helm chart included).

> **Status:** Early-stage open source release. APIs and data models may still change.

## Features

- CRM: business partners, contacts, leads, deals, campaigns
- Quote → contract → invoice flow with journal, payment matching, fees and refunds
- E-invoicing: XRechnung / UBL and ZUGFeRD (CII) export
- Agent API: versioned REST/OpenAPI with context endpoint, idempotent commands, event cursor and next actions
- Access control: local users with roles (admin, sales, accounting, read-only), scoped agent tokens, optional OIDC (Keycloak etc.) with PKCE
- Backups and restore, health/readiness probes, container image and Helm chart
- No external runtime dependencies: no Node, no separate database server, no CDN

## Tech stack

Rust (axum, tokio, sqlx/SQLite) · embedded HTML/CSS/JS UI · Helm · Playwright browser checks

## Quick start

```sh
cargo build --locked --release --workspace

# Standalone demo with fictitious data (loopback only, no login)
./target/release/brilliantis --demo --port 4320
# → http://127.0.0.1:4320

# Your own instance
./target/release/brilliantis --database data/company.sqlite \
  --init-admin admin@example.org --password-file /secure/path/admin-password
./target/release/brilliantis --database data/company.sqlite --port 4321
```

Run `--help` for all options. Container: see `Dockerfile`; Kubernetes: `deploy/helm/brilliantis`.

## AI agents

Brilliantis does not embed or require any specific AI vendor. Agents — whether built on OpenAI, Anthropic,
Mistral, a local model via Ollama/LM Studio, or a plain script — interact through the documented REST/OpenAPI
interface (`/api/v1/agent/context`, records, commands). Access is granted with scoped agent tokens.

## Configuration

All configuration is read from environment variables or local config files that are **never** committed.
Copy the provided example files (e.g. `.env.example`) and fill in your own values. The examples contain
placeholders only — no real credentials, hostnames or customer data ship with this repository.

## Project layout

| Path | Content |
| --- | --- |
| `src/` | Rust core: domain, finance, commerce, documents, auth/OIDC, API |
| `static/` | Embedded web UI |
| `migrations/` | SQLite schema migrations |
| `distributions/` | `standalone` profile and the optional `neurofunken` distribution package |
| `deploy/helm/` | Helm chart |
| `tests/`, `scripts/` | Integration tests and browser/container checks |
| `docs/` | [Public concept](docs/CONCEPT.md) and additional documentation |

## Contributing

Contributions are welcome — bug reports, documentation, tests and features alike.
Please read [CONTRIBUTING.md](CONTRIBUTING.md) and our [Code of Conduct](CODE_OF_CONDUCT.md) first.
Security issues: please follow [SECURITY.md](SECURITY.md) and do **not** open a public issue.

## License

Released under the [MIT License](LICENSE). © 2026 Hanns Leonhard Osterbrink / neurofunken.

---

## Kurzfassung (Deutsch)

Brilliantis ist ein agentenorientiertes Open-Source-CRM und -ERP in Rust: Vertrieb, Angebote, Verträge, Rechnungen (inkl. XRechnung/ZUGFeRD), Zahlungen und Kampagnen in einem Arbeitsbereich. Menschen arbeiten in der eingebetteten Oberfläche, KI-Agenten über dieselbe REST/OpenAPI-Schnittstelle. Ein einzelnes Binary mit SQLite, optional Helm/Kubernetes. Schnellstart: `cargo build --release --workspace` und `./target/release/brilliantis --demo`.

Beiträge sind willkommen – siehe [CONTRIBUTING.md](CONTRIBUTING.md). Sicherheitslücken bitte vertraulich
gemäß [SECURITY.md](SECURITY.md) melden. Lizenz: MIT.

Ausführliche deutschsprachige Dokumentation: [docs/README.de.md](docs/README.de.md).

<p align="center"><sub>Built with care in Darmstadt by <a href="https://neurofunken.de">neurofunken</a>.</sub></p>
