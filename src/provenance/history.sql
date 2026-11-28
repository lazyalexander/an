SELECT n.node,h.body FROM pv_node n JOIN pv_node_history h
ON (h.task,h.node,h.cursor)=(n.task,n.node,(
    SELECT max(cursor) FROM pv_node_history
    WHERE task=n.task AND node=n.node AND cursor<=?2
))
WHERE n.task=?1 AND n.first_cursor<=?2 ORDER BY n.node
