-- Teams: groups of users (people or service accounts) that can hold role
-- assignments (design doc §11.3). Members get the team's grants on top of
-- their own. Teams are instance-wide, like users.

CREATE TABLE teams (
    id           uuid PRIMARY KEY,
    name         text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    description  text NOT NULL DEFAULT '' CHECK (length(description) <= 500),
    version      integer NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX teams_name_unique ON teams (lower(name));

CREATE TRIGGER teams_touch
    BEFORE UPDATE ON teams
    FOR EACH ROW EXECUTE FUNCTION huxley_touch_updated_at();

CREATE TABLE team_members (
    team_id   uuid NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    user_id   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    added_at  timestamptz NOT NULL DEFAULT now(),
    added_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (team_id, user_id)
);

CREATE INDEX team_members_user ON team_members (user_id);

-- An assignment goes to a user or a team, never both.
ALTER TABLE role_assignments ALTER COLUMN user_id DROP NOT NULL;
ALTER TABLE role_assignments
    ADD COLUMN team_id uuid REFERENCES teams (id) ON DELETE CASCADE,
    ADD CONSTRAINT role_assignments_one_principal CHECK ((user_id IS NULL) <> (team_id IS NULL));

CREATE UNIQUE INDEX role_assignments_team_unique ON role_assignments (team_id, role_id, org_id)
    WHERE team_id IS NOT NULL;
CREATE INDEX role_assignments_team ON role_assignments (team_id) WHERE team_id IS NOT NULL;

-- Owner is granted to individual people only, never to a team.
CREATE FUNCTION huxley_owner_not_team() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.team_id IS NOT NULL AND EXISTS (SELECT 1 FROM roles WHERE id = NEW.role_id AND key = 'owner') THEN
        RAISE EXCEPTION 'the Owner role cannot be assigned to a team'
            USING ERRCODE = 'check_violation', CONSTRAINT = 'role_assignments_owner_not_team';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER role_assignments_owner_not_team
    BEFORE INSERT OR UPDATE ON role_assignments
    FOR EACH ROW EXECUTE FUNCTION huxley_owner_not_team();
