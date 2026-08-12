-- V19: Normalize qualified runtime-dependency edges to bare output names.
--
-- `depends_on` historically stored the dependency reference verbatim, so a
-- dep declared as `plan:output` was persisted with its qualifier. Dependent
-- and orphan queries match `depends_on` literally against deployed part
-- names, which made qualified edges invisible at removal time (removal of
-- the target was neither blocked nor cascaded). Deployed part names are
-- globally unique, so the bare output name is the canonical edge key; new
-- writes are normalized at deploy time and this migration rewrites existing
-- rows to match.

UPDATE dependencies
SET depends_on = substr(depends_on, instr(depends_on, ':') + 1)
WHERE instr(depends_on, ':') > 0;
