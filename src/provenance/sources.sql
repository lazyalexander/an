SELECT source,source_version,source_block,relation FROM pv_edge
WHERE artifact=? AND version=? AND block=? ORDER BY source,source_version,source_block,relation
