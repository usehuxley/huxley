# ADR-071: The forecast is first-year value by forecast category against monthly or quarterly quotas, and sales reporting uses the analytics sources

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 10 asks for forecasting (weighted pipeline, categories, quotas) and for sales dashboards and reports. Opportunities already carry a forecast category, a probability, an expected close date, and one-time and monthly values. Reporting already has a governed query layer with dashboards (ADR-041).

## Decision

- **One value measure:** first-year value, meaning one-time plus twelve months of recurring. The pipeline, quotes and dashboards already use it.
- **The forecast**, for a period and per owner and currency, from the caller's scope:
  - **won:** status won, closed in the period;
  - **commit:** open, expected to close in the period, in the commit category;
  - **best case:** commit plus best case;
  - **pipeline:** every open deal expected in the period except those left out;
  - **weighted:** the open deals' value times probability;
  - **attainment:** won over quota.
- **Quotas** are per salesperson, for a month or a calendar quarter, in a currency. They're kept at the organization the person sells for and set by `sales:configure`. A period's quota is the sum of quotas starting in it, so a year sums its quarters. Setting both monthly and quarterly quotas would double-count; the docs say to pick one.
- **Reporting:**
  - `opportunities`, `quotes` and `sales_activities` join the analytics catalog. The opportunities source's range field is when the deal closed, so a dashboard's range means "won or lost in".
  - A prebuilt **Sales** dashboard ships, and is listed only while the sales module is on. Prebuilt dashboards can now name a module; the existing ones don't, so they're listed as before.

## Consequences

- Teams' quotas (summed) and quota by product line can come later without changing the measure.
- Forecast snapshots (how the forecast moved week to week) aren't kept yet. The audit log and stage history hold enough to add them.
