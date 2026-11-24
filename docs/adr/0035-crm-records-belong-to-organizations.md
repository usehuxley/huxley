# ADR-035: CRM records belong to organizations; contacts can link to several

- **Status:** Accepted (implementer's design, per the maintainer's "you choose")
- **Date:** 2026-11-25

## Context

Phase 5 starts the `crm` module (design doc §8.1).

- Client organizations already *are* companies. The tree, permissions and row-level security are built on them.
- Companies need sites, contacts and a few company details.
- A contact can belong to several companies, one of them primary.
- Like every module, `crm` can be switched off, and the platform must work without it.

## Decision

- **Company details** live in `crm_companies`, one row per organization: relationship (client, vendor, partner, prospect), status, website, phone and notes. Organizations themselves are unchanged, so turning the module off leaves the core untouched.
- **Sites** (`crm_sites`) belong to an organization: name, postal address, an IANA timezone, opening hours per weekday, phone, and whether it's the primary site.
- **Contacts** (`crm_contacts`) belong to their **primary company**: the `org_id` that row-level security and permissions use.
  - **Links:** `crm_contact_companies` links a contact to further companies, each link with its own job title, and exactly one link is the primary.
  - **Visibility:** a contact shows on a linked company's list only for people who can see its primary company too. Contacts aren't shared further than their primary company's audience.
  - **Identities:** contacts can carry external identities (`external_ids`, e.g. `{"entra": "<object id>"}`) for matching people from directories and email, and later a portal user.
- **Common features:** contacts and sites are entity types, so they get custom fields and tags like organizations. Changes are audited and emit domain events (`contact.*`, `site.*`, `company.updated`).
- **Permissions:**
  - `crm:read` lets people view companies, contacts and sites.
  - `crm:manage` lets them create and change contacts and sites, and edit company details.
  - Technicians get both, and the read-only role gets `crm:read`. Owners and administrators already have everything.
- **Module gate:** every CRM route checks that the `crm` module is active at the organization concerned. With it off, the routes answer 404 and nothing else changes.

## Consequences

- No change to how organizations, row-level security or permissions work.
- Phase 5's later parts can reference contacts and sites, such as ticket requesters, mailbox contact matching and SLA business hours from a site's timezone and hours.
- Company types and statuses are fixed lists for now. Configurable lists can come later if they're needed.
