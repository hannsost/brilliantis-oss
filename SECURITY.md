<!-- neurofunken-oss-security -->
# Security Policy

## Supported versions

Only the latest release / the `main` branch of Brilliantis receives security fixes.

## Reporting a vulnerability

Please **do not** open public issues for security problems.

- Preferred: use GitHub's **private vulnerability reporting** ("Security" → "Report a vulnerability")
  or a **confidential issue** on GitLab.
- Alternatively email **security@neurofunken.de** with a description, affected version/commit,
  reproduction steps and, if possible, a proof of concept.

We aim to acknowledge reports within 3 working days and to provide a fix or mitigation plan within 30 days.
Please give us a reasonable time to fix the issue before public disclosure. We are happy to credit reporters.

## Scope and good practice

- Never put real credentials into configuration files that are committed; use environment variables or a secret manager.
- Example files (`*.example`) contain placeholders only.
- If you discover a credential that was accidentally published in this repository, report it as a vulnerability.
