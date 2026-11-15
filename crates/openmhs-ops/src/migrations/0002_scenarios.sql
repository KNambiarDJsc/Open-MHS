-- Scenario ids (RFC 0005 0.1 addition): attempts that share one are replicates of the same situation.
ALTER TABLE events ADD COLUMN scenario_id TEXT;
UPDATE events SET scenario_id = json_extract(body, '$.subject.scenario_id')
    WHERE json_extract(body, '$.subject.scenario_id') IS NOT NULL;
ALTER TABLE task_attempts ADD COLUMN scenario_id TEXT;
