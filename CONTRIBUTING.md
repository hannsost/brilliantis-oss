# Contributing to Brilliantis

Thanks for your interest in improving Brilliantis! This document explains how to propose changes.

## Ground rules

- Be respectful — this project follows the [Code of Conduct](CODE_OF_CONDUCT.md).
- Never commit secrets, real customer data, personal data or production configuration.
  Use placeholders and the provided `*.example` files.
- Report security vulnerabilities privately (see [SECURITY.md](SECURITY.md)).

## Development workflow

1. Fork the repository and create a topic branch from `main` (`feat/…`, `fix/…`, `docs/…`).
2. Make focused commits with clear messages (Conventional Commits are appreciated, e.g. `fix(api): handle empty payload`).
3. Add or update tests for behaviour changes.
4. Run the checks locally before opening a pull/merge request:

```sh
cargo fmt --all --check
cargo test --locked --workspace
```

5. Open a pull request (GitHub) or merge request (GitLab) and describe *what* and *why*.
   Link related issues. CI must be green before review.

## Coding style

Rust code is formatted with `rustfmt`; keep the core independent of the `distributions/neurofunken` package.

## Licensing of contributions

By contributing you agree that your contributions are licensed under the project's [MIT License](LICENSE).
