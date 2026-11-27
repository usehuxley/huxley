-- Expenses on invoices (design doc §8.16, ADR-082): approved billable
-- expenses become invoice lines in billing runs (marked up), each billed
-- once; and project cost includes expenses.

ALTER TABLE billing_invoice_lines DROP CONSTRAINT billing_invoice_lines_kind_check;
ALTER TABLE billing_invoice_lines ADD CONSTRAINT billing_invoice_lines_kind_check
    CHECK (kind IN ('time', 'product', 'recurring', 'project', 'expense', 'manual'));

ALTER TABLE billing_billed DROP CONSTRAINT billing_billed_source_type_check;
ALTER TABLE billing_billed ADD CONSTRAINT billing_billed_source_type_check
    CHECK (source_type IN ('time_entry', 'sales_order_line', 'agreement_line', 'project_fee', 'project_milestone',
                           'expense'));

-- A project's expense budget (cost), beside its hours budget.
ALTER TABLE prj_projects ADD COLUMN cost_budget numeric(18,4) CHECK (cost_budget >= 0);
