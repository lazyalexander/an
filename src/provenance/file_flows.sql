-- Receiving a file may influence any output of that attempt. Only a
-- confirmed handoff can extend this walk. All results have file precision.
WITH RECURSIVE files(revision,producer,input) AS (
    VALUES(?2,?3,?4)
    UNION
    SELECT p.revision,p.attempt,NULL
    FROM files f JOIN pv_flow h ON h.task=?1 AND h.phase='handed_off'
      AND (h.revision,h.source_attempt,h.source_input) IS (f.revision,f.producer,f.input)
    JOIN pv_product p ON (p.task,p.attempt)=(h.task,h.attempt)
), observed(seq,phase,attempt,target,delivery,time_ms) AS (
    SELECT h.seq,h.phase,h.attempt,h.target,h.delivery,h.time_ms
    FROM files f JOIN pv_flow h ON h.task=?1
      AND (h.revision,h.source_attempt,h.source_input) IS (f.revision,f.producer,f.input)
    WHERE h.phase IN ('authorized','handed_off','published')
)
SELECT * FROM observed
UNION
SELECT x.seq,x.phase,x.attempt,x.target,x.delivery,x.time_ms FROM pv_flow x
JOIN observed pub ON pub.phase='published' AND x.delivery=pub.delivery
WHERE x.task=?1 AND x.phase IN ('exported','unknown')
ORDER BY seq
