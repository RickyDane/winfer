ALTER TABLE people ADD COLUMN first_name TEXT NOT NULL DEFAULT '';
ALTER TABLE people ADD COLUMN last_name TEXT NOT NULL DEFAULT '';
UPDATE people SET name = trim(name);
UPDATE people SET
    first_name = CASE WHEN instr(name, ' ') = 0 THEN name
                      ELSE trim(rtrim(name, replace(name, ' ', ''))) END,
    last_name  = CASE WHEN instr(name, ' ') = 0 THEN ''
                      ELSE substr(name, length(rtrim(name, replace(name, ' ', ''))) + 1) END;
ALTER TABLE people DROP COLUMN name;
CREATE INDEX idx_people_last_first ON people(last_name, first_name);
