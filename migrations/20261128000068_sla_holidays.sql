-- SLA holidays (design doc §8.2): days a policy's clocks don't count, like
-- public holidays. `[{"date": "2026-12-25", "name": "Christmas Day"}, …]`.
ALTER TABLE sd_sla_policies ADD COLUMN holidays jsonb NOT NULL DEFAULT '[]'::jsonb;
