# Clients and contacts

The **Clients and contacts** module (`crm`) adds company details, sites and contacts to your organizations. Client organizations *are* your companies: everything here belongs to one, and is visible to the people who can see that organization.

Turn it on under **Settings → Modules**. A provider organization can turn it off for itself and its clients. With it off, nothing here exists for them, and the rest of Huxley is unchanged.

## Company details

Each organization has a few company details:

- **Relationship:** client, vendor, partner or prospect.
- **Status:** active, inactive, onboarding or offboarding.
- **Contact details and notes:** website, phone and notes.

## Sites

A company's locations: a name, a postal address, a **timezone** and **opening hours** per weekday (for example `"mon": [["09:00", "12:30"], ["13:30", "17:00"]]`, where a missing day is closed). One site is the company's primary site, and its first site is primary automatically. Archived sites stay on the contacts that were at them.

## Contacts

The people at your companies: names, email, phone and mobile, a site, and identities elsewhere, such as their Entra ID object id (`{"entra": "…"}`), for matching them later.

- **Several companies:** a contact can belong to several companies, each with its own job title. One is the **primary** company: the contact belongs to it, and only people who can see that company can see the contact, even where it's listed at another.
- **Changing companies:** this needs `crm:manage` at every company involved, before and after the change.
- **Custom fields and tags:** contacts and sites take them like organizations do (**Settings → Custom fields**).
- **History:** changes are audited, and emit events (`contact.created`, `contact.updated`, `contact.archived`, `site.*`, `company.updated`) that workflows can be triggered by.

## Permissions

| Permission | Allows |
|---|---|
| `crm:read` | Viewing company details, contacts and sites |
| `crm:manage` | Creating and changing contacts and sites, and editing company details |

Technicians have both, and the Read-only role has `crm:read`.

## The API

| | |
|---|---|
| `GET`/`PUT /api/v1/orgs/{id}/company` | Company details. `PUT` takes the `version` you read (0 the first time). |
| `GET`/`POST /api/v1/orgs/{id}/sites` | An organization's sites |
| `GET`/`PUT`/`DELETE /api/v1/sites/{id}` | A site (`DELETE` archives it) |
| `GET`/`POST /api/v1/orgs/{id}/contacts` | Contacts at an organization (`search`, `includeArchived`, `limit`, `offset`) |
| `GET`/`PUT`/`DELETE /api/v1/contacts/{id}` | A contact (`DELETE` archives it) |
| `PUT /api/v1/contacts/{id}/companies` | `{ version, companies: [{ orgId, jobTitle, isPrimary }] }` |
