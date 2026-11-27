# Xero

The **Xero** pack syncs approved invoices and credit memos to a Xero organisation (see [Invoicing](./billing.md#accounting-sync)).

## Setting it up

1. In the [Xero developer portal](https://developer.xero.com/app/manage), create a **Web app**. Add Huxley's OAuth redirect URL (shown when you add the connection) as its redirect URI.
2. Install **Xero** from **Packs** at your MSP.
3. Add a connection to the **Xero** integration with key `xero` and the app's client ID and secret, then sign in. Use a Xero user with the Standard or Adviser role.

   The integration asks for offline access, accounting transactions, contacts and settings. If your app was created with Xero's newer, narrower scopes, edit the connection's scopes to their equivalents for invoices, credit notes, contacts and settings.
4. Find the organisation's tenant ID: run the integration's **listConnections** action, or use the `tenantId` Xero shows for the connection. Set it as the organization variable `xero_tenant_id` at your MSP.
5. Publish **Fetch Xero lists** and run it. It reads customer contacts, items with a code, tax rates and tracking options into datasets, then runs nightly.
6. Publish **Sync invoices to Xero**, set **Settings → Billing → Accounting sync** to `xero`, and match under **Billing → Accounting**. Tracking options are what Huxley calls classes.

## What it does

When an invoice is to be synced, the workflow creates an authorised **invoice** in Xero (or an authorised **credit note**, for a credit memo):
- the matched contact;
- one line item per invoice line, with the matched item code, quantity, unit price and tax rate, and the tracking option if one is matched;
- the invoice number;
- the dates and the currency;
- amounts exclusive of tax.

Xero's ID and number are recorded on the invoice, with a link to open it in Xero, and the invoice is marked sent. If Xero refuses it, its validation messages go to the sync error queue.

Items need a sales account in Xero, since lines carry the item code and not an account code. Xero calculates tax. Huxley doesn't.
