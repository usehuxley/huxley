-- The `workforce` module's base (design doc §8.15): skills and
-- certifications, what people have, and what work requires. Instance-wide
-- records about the MSP's people (users), so not split by organization.

CREATE TABLE wf_skills (
    id          uuid PRIMARY KEY,
    name        text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    category    text NOT NULL DEFAULT '' CHECK (length(category) <= 100),
    description text NOT NULL DEFAULT '' CHECK (length(description) <= 2000),
    version     integer NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    archived_at timestamptz
);
CREATE UNIQUE INDEX wf_skills_name ON wf_skills (lower(name)) WHERE archived_at IS NULL;

-- 1 learning, 2 working, 3 proficient, 4 expert.
CREATE TABLE wf_user_skills (
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    skill_id    uuid NOT NULL REFERENCES wf_skills (id) ON DELETE CASCADE,
    level       smallint NOT NULL CHECK (level BETWEEN 1 AND 4),
    -- Who verified this level (none: self-assessed), and when.
    verified_by uuid REFERENCES users (id) ON DELETE SET NULL,
    verified_at timestamptz,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, skill_id)
);

CREATE TABLE wf_certifications (
    id              uuid PRIMARY KEY,
    vendor          text NOT NULL CHECK (length(btrim(vendor)) BETWEEN 1 AND 100),
    name            text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    code            text NOT NULL DEFAULT '' CHECK (length(code) <= 50),
    -- How long it's valid once earned; none: it doesn't expire.
    validity_months integer CHECK (validity_months BETWEEN 1 AND 240),
    description     text NOT NULL DEFAULT '' CHECK (length(description) <= 2000),
    version         integer NOT NULL DEFAULT 1,
    created_at      timestamptz NOT NULL DEFAULT now(),
    archived_at     timestamptz
);
CREATE UNIQUE INDEX wf_certifications_name ON wf_certifications (lower(vendor), lower(name))
    WHERE archived_at IS NULL;

CREATE TABLE wf_user_certifications (
    id                uuid PRIMARY KEY,
    user_id           uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    certification_id  uuid NOT NULL REFERENCES wf_certifications (id) ON DELETE CASCADE,
    earned_on         date NOT NULL,
    expires_on        date,
    credential_id     text NOT NULL DEFAULT '' CHECK (length(credential_id) <= 200),
    verify_url        text CHECK (verify_url IS NULL OR verify_url ~ '^https://'),
    -- When the expiry warning and the expiry were announced (once each).
    warned_at         timestamptz,
    expired_at        timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_on IS NULL OR expires_on >= earned_on)
);
CREATE INDEX wf_user_certifications_user ON wf_user_certifications (user_id);
CREATE INDEX wf_user_certifications_expiry ON wf_user_certifications (expires_on)
    WHERE expired_at IS NULL;

-- What work needs: a skill at a level, or a certification (a hard
-- requirement), on a queue or a ticket type.
CREATE TABLE wf_requirements (
    id                uuid PRIMARY KEY,
    target_type       text NOT NULL CHECK (target_type IN ('queue', 'ticket_type')),
    target_id         uuid NOT NULL,
    skill_id          uuid REFERENCES wf_skills (id) ON DELETE CASCADE,
    min_level         smallint CHECK (min_level BETWEEN 1 AND 4),
    certification_id  uuid REFERENCES wf_certifications (id) ON DELETE CASCADE,
    CHECK ((skill_id IS NULL) <> (certification_id IS NULL)),
    CHECK (skill_id IS NULL OR min_level IS NOT NULL)
);
CREATE UNIQUE INDEX wf_requirements_one ON wf_requirements
    (target_type, target_id, coalesce(skill_id, certification_id));

-- Technicians see the people they work with and keep their own skills
-- and certifications; read-only users can see them.
UPDATE roles SET permissions = permissions || '{workforce:read}'
 WHERE key IN ('technician', 'read_only') AND is_system;
