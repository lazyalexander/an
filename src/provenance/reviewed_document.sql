SELECT EXISTS (
    SELECT 1 FROM pv_version v
    JOIN pv_artifact doc ON doc.id = v.artifact AND doc.kind = 'document'
    JOIN pv_artifact report ON report.id = ?3 AND report.kind = 'report'
    WHERE v.artifact = ?1 AND v.version = ?2
      AND NOT EXISTS (
        SELECT 1 FROM pv_block b
        WHERE b.artifact = v.artifact AND b.version = v.version
          AND NOT EXISTS (
            SELECT 1 FROM pv_edge e
            WHERE (e.artifact,e.version,e.block) = (?3,?4,?5)
              AND (e.source,e.source_version,e.source_block) = (b.artifact,b.version,b.block)
              AND e.relation = 'verifies'
          )
      )
)
