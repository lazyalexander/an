SELECT artifact,version,block,relation FROM pv_edge
WHERE source=? AND source_version=? AND source_block=? ORDER BY artifact,version,block,relation
