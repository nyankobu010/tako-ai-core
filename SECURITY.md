# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately via GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
or email `TODO(community): security@<placeholder>`.

We follow a **90-day coordinated disclosure** window from the date the report
is acknowledged. We will work with you on a fix and a public advisory.

Please **do not** open public issues for security reports.

## Supply chain

- Releases are built in GitHub Actions via [PyO3/maturin-action](https://github.com/PyO3/maturin-action).
- Wheels and sdists are signed with [Sigstore](https://www.sigstore.dev/) (keyless / OIDC).
- We publish to PyPI via Trusted Publishing; no long-lived API tokens.
- `cargo audit` and `pip-audit` run in CI on every push and on a daily schedule.
- Dependabot is enabled for cargo, pip, and github-actions ecosystems.

## Scope

In scope: any vulnerability in `tako` source code, build process, or release artefacts.
Out of scope: vulnerabilities in upstream dependencies (please report to the upstream).
