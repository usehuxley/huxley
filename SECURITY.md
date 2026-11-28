# Security policy

Security is a core requirement for Huxley. MSPs trust it with credentials for their clients' systems.

## Reporting a vulnerability

**Don't open a public issue.** Report privately:

- through GitHub's [private vulnerability reporting](https://github.com/usehuxley/huxley/security/advisories/new) (preferred), or
- by email to **security@usehuxley.io**.

Please include:
- the affected version or commit;
- steps to reproduce;
- the impact you believe it has.

A proof of concept helps. If you found it while testing an instance you run, say which configuration.

## What to expect

| | |
|---|---|
| **Acknowledgement** | Within 3 business days. |
| **Assessment** | Within 10 business days. We confirm the issue (or explain why we don't), tell you its severity, and agree a timeline. |
| **Fix** | Critical and high severity: as fast as we can, aiming for 30 days. Others: in a coming release. |
| **Disclosure** | Coordinated with you. We publish an advisory when a fix is available, normally within 90 days of your report. We'll tell you if we need longer, and why. |

We credit reporters in the advisory, unless you'd rather we didn't.

## Testing safely

When looking for vulnerabilities:

- **Test only instances you run yourself,** or ones you have the operator's written permission to test. Never test an MSP's or anyone else's instance without their permission.
- **Don't access, change or delete other people's data.** Use test accounts and data. If you reach real data by accident, stop and tell us, without keeping it.
- **No denial of service,** spam or social engineering, and no physical attacks.
- **Give us reasonable time** to fix an issue before you disclose it.

We won't pursue or support legal action against anyone who acts in good faith within these guidelines, and we'll make that clear if someone else does.

## Supported versions

Huxley is pre-release. Until 1.0, only the latest `main` gets security fixes. From 1.0, this section will list the supported releases.

## Scope

**In scope:** this repository's server, apps, script runner, container images and deployment templates (Compose, Helm, Terraform, marketplace images).

**Out of scope:**
- vulnerabilities in third-party systems that Huxley's integrations reach;
- a self-hosted instance's own configuration, such as weak TLS settings or an exposed database, unless Huxley's defaults or documentation lead to it;
- denial of service through resource exhaustion on instances you control.

## How Huxley is secured

`docs/security.md` has the threat model (STRIDE by component), our review against OWASP ASVS Level 2 and the risks we accept. In CI:
- dependencies are audited (`cargo deny`);
- container images are scanned, and release images are signed with cosign (keyless);
- every change runs a backup-and-restore drill and the security regression tests.

The kit for an external penetration test is in `security/pentest/`.
