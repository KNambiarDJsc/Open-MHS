-- The shape of an incident (ADR-0035, rules shape-1): derived, filled by the next rebuild.
ALTER TABLE incidents ADD COLUMN shape TEXT;
ALTER TABLE incidents ADD COLUMN shape_signature TEXT;
CREATE INDEX incidents_signature ON incidents(tenant, shape_signature, opened_ns);
