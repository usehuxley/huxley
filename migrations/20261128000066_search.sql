-- Global search (design doc §8.2): trigram indexes for matching parts of
-- names and titles.

CREATE INDEX sd_tickets_title_trgm ON sd_tickets USING gin (title gin_trgm_ops);
CREATE INDEX crm_contacts_name_trgm ON crm_contacts
    USING gin ((first_name || ' ' || last_name || ' ' || coalesce(email, '')) gin_trgm_ops);
CREATE INDEX organizations_name_trgm ON organizations USING gin (name gin_trgm_ops);
CREATE INDEX workflows_name_trgm ON workflows USING gin (name gin_trgm_ops);
CREATE INDEX users_name_trgm ON users USING gin ((display_name || ' ' || email) gin_trgm_ops);
