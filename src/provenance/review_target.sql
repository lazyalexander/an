WITH RECURSIVE lineage(artifact, version, block) AS (
    SELECT b.artifact, b.version, b.block
    FROM pv_published p JOIN pv_block b USING (artifact, version)
    WHERE p.delivery = ?1 AND p.workspace = ?4
    UNION
    SELECT e.source, e.source_version, e.source_block
    FROM lineage l JOIN pv_edge e USING (artifact, version, block)
)
SELECT EXISTS (
    SELECT 1 FROM lineage l JOIN pv_artifact a ON a.id = l.artifact
    JOIN objects t ON t.kind = 'task' AND t.id = a.owner
    WHERE l.artifact = ?2 AND l.version = ?3 AND a.kind = 'decision'
      AND json_extract(t.body, '$.workspace') = ?4
      AND json_extract(t.body, '$.closed') = 1
)
