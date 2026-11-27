# QuickBooks Online

The **QuickBooks Online** pack syncs approved invoices and credit memos to a QuickBooks Online company (see [Invoicing](./billing.md#accounting-sync)).

## Setting it up

1. In the [Intuit developer portal](https://developer.intuit.com), create an app with the **Accounting** scope. Add Huxley's OAuth redirect URL (shown when you add the connection) to its redirect URIs.
2. Install **QuickBooks Online** from **Packs** at your MSP.
3. Add a connection to the **QuickBooks Online** integration:
   - key `quickbooks`;
   - the app's client ID and secret;
   - **company** set to your company (realm) ID, found under **Settings → Additional info** in QuickBooks.

   Sign in as a QuickBooks admin. For a sandbox company, set **host** to `sandbox-quickbooks.api.intuit.com`, and set the organization variable `quickbooks_app_host` to `app.sandbox.qbo.intuit.com` so links open the sandbox.
4. Publish **Fetch QuickBooks lists** and run it. It reads customers, items, tax codes and classes into datasets (up to 1,000 of each), then runs nightly.
5. Publish **Sync invoices to QuickBooks Online**, set **Settings → Billing → Accounting sync** to `quickbooks`, and match under **Billing → Accounting**.

## What it does

When an invoice is to be synced, the workflow creates an **Invoice** in QuickBooks (or a **Credit Memo**, for a credit memo):
- the matched customer;
- one sales line per invoice line, with the matched item, quantity, price and tax code, and the class if one is matched;
- the invoice number as the document number;
- the issue and due dates and the currency;
- the invoice's notes as the customer memo.

QuickBooks' ID and number are recorded on the invoice, with a link to open it in QuickBooks, and the invoice is marked sent. If QuickBooks refuses it, its reason goes to the sync error queue, for example "Duplicate Document Number Error".

QuickBooks calculates tax from the tax codes. Huxley doesn't.

## Payment status

Publish **Update invoice status from QuickBooks Online**. Every hour it reads what's still due on synced invoices that aren't paid, and records it. The first time, it also fetches each invoice's view-and-pay link (QuickBooks Payments must be on) for the client portal. See [payment status and credit hold](./billing.md#payment-status-and-credit-hold).
