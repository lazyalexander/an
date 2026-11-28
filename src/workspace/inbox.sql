SELECT i.metadata,b.text
FROM pv_inbox i JOIN pv_frozen_text b USING(artifact,version,block)
WHERE i.workspace=? ORDER BY i.seq
