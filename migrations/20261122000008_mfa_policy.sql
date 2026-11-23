-- Requiring multi-factor authentication (design doc §11.2: "enforceable per
-- role or org").
--
-- A user must have a second factor if they hold a role with require_mfa, or
-- can reach an organization with require_mfa: they're assigned at it, at a
-- descendant of it, or at an ancestor with inheritance. Setting it on the
-- root organization applies to every user.

ALTER TABLE roles ADD COLUMN require_mfa boolean NOT NULL DEFAULT false;
ALTER TABLE organizations ADD COLUMN require_mfa boolean NOT NULL DEFAULT false;

CREATE INDEX organizations_require_mfa ON organizations USING gist (path) WHERE require_mfa;
