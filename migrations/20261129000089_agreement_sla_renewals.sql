-- Agreements can bind an SLA policy (their client's new tickets get it),
-- and are tracked for renewal: contract.renewal_due once per term, and
-- automatic renewal rolls the term on.
ALTER TABLE contract_agreements
    ADD COLUMN sla_policy_id uuid REFERENCES sd_sla_policies (id) ON DELETE SET NULL,
    ADD COLUMN renewal_announced_for date;
CREATE INDEX contract_agreements_renewal ON contract_agreements (ends_on) WHERE status = 'active';
