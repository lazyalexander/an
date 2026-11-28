WITH notices AS (
    SELECT n.decision, n.recipient,
           CASE WHEN max(a.status='received') THEN 'received'
                WHEN max(a.status='unknown') THEN 'unknown'
                WHEN max(a.status='failed') THEN 'failed'
                ELSE 'unclaimed' END AS status
    FROM pv_notice n LEFT JOIN pv_notice_attempt a ON a.notice=n.id
    WHERE n.decision IN (?1, ?2)
    GROUP BY n.decision, n.recipient
)
SELECT recipient,
       coalesce(max(CASE WHEN decision=?1 THEN status END), 'not_requested'),
       coalesce(max(CASE WHEN decision=?2 THEN status END), 'not_requested')
FROM notices GROUP BY recipient ORDER BY recipient
