-- Actions (design doc §8.2, Client 360): forms offered on a client,
-- contact or ticket, which start their workflows with that record as
-- context.
ALTER TABLE forms ADD COLUMN action_for text CHECK (action_for IN ('client', 'contact', 'ticket'));
-- What a submission was run on: `{ "client": …, "contact": …, "ticket": … }`.
ALTER TABLE form_submissions ADD COLUMN context jsonb;
