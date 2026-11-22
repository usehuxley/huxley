# Security Policy

Security is a core requirement for Huxley. MSPs trust it with credentials for their clients' systems.

## Reporting a vulnerability

**Do not open a public issue.** Report vulnerabilities privately through GitHub's [private vulnerability reporting](https://github.com/usehuxley/huxley/security/advisories/new) or by email to **security@usehuxley.com**.

Please include affected versions, steps to reproduce, and the impact you believe it has. We aim to acknowledge reports within 3 business days and to agree on a disclosure timeline with you. We credit reporters who want credit.

## Supported versions

Huxley is pre-release. Until 1.0, only the latest `main` receives security fixes.

## Scope

In scope: this repository's server, apps, container images, and deployment templates. Out of scope: vulnerabilities in third-party systems reached through integrations, and denial of service through resource exhaustion on self-hosted instances you control.

## Practices

The design targets OWASP ASVS Level 2. Security controls are described in [the design document](docs/DESIGN_DOCUMENT.md) §12. Dependencies are audited in CI, container images are scanned with Grype, and release images are signed with cosign.
