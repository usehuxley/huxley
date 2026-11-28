-- Backups must restore completely (docs/backup.md). pg_dump writes a
-- trigger's WHEN condition as-is and restores it with an empty search_path,
-- where `IS DISTINCT FROM` on ltree can't find the operator (it lives in the
-- extension's schema, and that form can't be schema-qualified). So
-- organizations_move_subtree was silently left out of every restore, and
-- moving an organization afterwards wouldn't have updated its subtree.
-- Comparing the paths as text means the same and needs only built-in
-- operators.

DROP TRIGGER organizations_move_subtree ON organizations;
CREATE TRIGGER organizations_move_subtree
    AFTER UPDATE OF parent_id ON organizations
    FOR EACH ROW WHEN (OLD.path::text IS DISTINCT FROM NEW.path::text)
    EXECUTE FUNCTION huxley_org_move_subtree();
