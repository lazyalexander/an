SELECT i.metadata,b.text,d.metadata
FROM pv_inbox i JOIN pv_frozen_text b USING(artifact,version,block)
LEFT JOIN pv_inbox_disposition d ON d.receipt=i.receipt
WHERE i.workspace=? ORDER BY i.seq
