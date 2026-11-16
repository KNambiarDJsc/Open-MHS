-- Wall-clock cost of an attempt (RFC 0005 0.1 addition, `cost.wall_s`): derived, filled by the next rebuild.
ALTER TABLE task_attempts ADD COLUMN wall_s REAL;
