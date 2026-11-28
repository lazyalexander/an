SELECT EXISTS (
    SELECT 1 FROM pv_edge e
    JOIN pv_artifact report ON report.id = e.artifact
    JOIN pv_artifact decision ON decision.id = e.source
    WHERE (e.artifact, e.version, e.block) = (?1, ?2, ?3)
      AND (e.source, e.source_version) = (?4, ?5)
      AND e.relation = 'verifies'
      AND report.kind = 'report' AND decision.kind = 'decision'
)
