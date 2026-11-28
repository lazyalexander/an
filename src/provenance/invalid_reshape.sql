WITH edits AS (
    SELECT block, source, source_version, source_block, relation FROM pv_edge
    WHERE artifact=?1 AND version=?2 AND relation IN ('split_from','merged_from')
)
SELECT block FROM edits e
WHERE source != ?1 OR source_version != ?2 - 1
   OR EXISTS (SELECT 1 FROM pv_block WHERE artifact=?1 AND version=?2-1 AND block=e.block)
   OR EXISTS (SELECT 1 FROM pv_block WHERE artifact=?1 AND version=?2 AND block=e.source_block)
   OR (relation='split_from' AND (
       (SELECT count(*) FROM edits WHERE block=e.block) != 1 OR
       (SELECT count(*) FROM edits WHERE source_block=e.source_block AND relation='split_from') < 2))
   OR (relation='merged_from' AND (
       (SELECT count(*) FROM edits WHERE source_block=e.source_block) != 1 OR
       (SELECT count(*) FROM edits WHERE block=e.block AND relation='merged_from') < 2))
LIMIT 1
