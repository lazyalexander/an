CREATE TABLE IF NOT EXISTS pv_artifact (
    id TEXT PRIMARY KEY, owner TEXT NOT NULL, kind TEXT NOT NULL, head INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS pv_version (
    artifact TEXT NOT NULL, version INTEGER NOT NULL, task TEXT NOT NULL,
    registered_cursor INTEGER NOT NULL, author TEXT NOT NULL, participant TEXT, material TEXT, evidence TEXT NOT NULL,
    PRIMARY KEY (artifact, version)
);
CREATE TABLE IF NOT EXISTS pv_block (
    artifact TEXT NOT NULL, version INTEGER NOT NULL, block TEXT NOT NULL,
    body TEXT NOT NULL, PRIMARY KEY (artifact, version, block)
);
CREATE TABLE IF NOT EXISTS pv_edge (
    artifact TEXT NOT NULL, version INTEGER NOT NULL, block TEXT NOT NULL,
    source TEXT NOT NULL, source_version INTEGER NOT NULL, source_block TEXT NOT NULL,
    relation TEXT NOT NULL,
    PRIMARY KEY (artifact, version, block, source, source_version, source_block, relation)
);
CREATE INDEX IF NOT EXISTS pv_consumers ON pv_edge(source, source_version, source_block);
CREATE TABLE IF NOT EXISTS pv_choice (
    artifact TEXT NOT NULL, version INTEGER NOT NULL, receipt TEXT NOT NULL UNIQUE,
    body TEXT NOT NULL, PRIMARY KEY(artifact,version)
);
CREATE TABLE IF NOT EXISTS pv_request (
    task TEXT NOT NULL, actor TEXT NOT NULL, key TEXT NOT NULL, digest TEXT NOT NULL,
    receipt TEXT NOT NULL, PRIMARY KEY(task,actor,key)
);
CREATE TABLE IF NOT EXISTS pv_point (
    task TEXT NOT NULL, cursor INTEGER NOT NULL, revision INTEGER NOT NULL,
    control TEXT, event TEXT, PRIMARY KEY(task,cursor)
);
CREATE TABLE IF NOT EXISTS pv_node_history (
    task TEXT NOT NULL,node TEXT NOT NULL,cursor INTEGER NOT NULL,body TEXT NOT NULL,
    PRIMARY KEY(task,node,cursor)
);
CREATE TABLE IF NOT EXISTS pv_node (
    task TEXT NOT NULL,node TEXT NOT NULL,first_cursor INTEGER NOT NULL,PRIMARY KEY(task,node)
);
CREATE TRIGGER IF NOT EXISTS pv_record_graph AFTER INSERT ON objects
WHEN NEW.kind='graph' AND json_type(NEW.body,'$.revision')='integer'
AND json_type(NEW.body,'$.events')='array' AND NOT EXISTS (
    SELECT 1 FROM pv_point WHERE task=NEW.id AND cursor=json_array_length(NEW.body,'$.events')
)
BEGIN
    INSERT INTO pv_point VALUES(NEW.id,json_array_length(NEW.body,'$.events'),
        json_extract(NEW.body,'$.revision'),json_extract(NEW.body,'$.control'),json_extract(NEW.body,'$.events[#-1]'));
    INSERT OR IGNORE INTO pv_node
        SELECT NEW.id,key,json_array_length(NEW.body,'$.events') FROM json_each(NEW.body,'$.nodes')
        WHERE NOT EXISTS (SELECT 1 FROM pv_node WHERE task=NEW.id AND node=key);
    INSERT INTO pv_node_history
        SELECT NEW.id,n.key,json_array_length(NEW.body,'$.events'),
            json_object('node',json(n.value),'activity',json(a.value),'driver',d.value) AS snapshot
        FROM json_each(NEW.body,'$.nodes') n JOIN json_each(NEW.body,'$.activity') a ON a.key=n.key
        LEFT JOIN json_each(NEW.body,'$.drivers') d ON d.key=n.key
        WHERE snapshot IS NOT (SELECT h.body FROM pv_node_history h
            WHERE h.task=NEW.id AND h.node=n.key ORDER BY cursor DESC LIMIT 1);
END;
CREATE TABLE IF NOT EXISTS pv_flow(
    seq INTEGER PRIMARY KEY, task TEXT NOT NULL, revision TEXT NOT NULL, phase TEXT NOT NULL,
    attempt TEXT NOT NULL, target TEXT NOT NULL, source_attempt TEXT, source_input TEXT, delivery TEXT, time_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS pv_revision_flows ON pv_flow(task,revision,seq);
CREATE INDEX IF NOT EXISTS pv_delivery_flows ON pv_flow(delivery,seq);
CREATE TABLE IF NOT EXISTS pv_export(
    seq INTEGER PRIMARY KEY, operation TEXT NOT NULL, attempt TEXT UNIQUE NOT NULL, result TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS pv_export_latest ON pv_export(operation,seq);
CREATE TABLE IF NOT EXISTS pv_published (
    delivery TEXT NOT NULL,binding TEXT NOT NULL,artifact TEXT NOT NULL,version INTEGER NOT NULL,
    workspace TEXT,PRIMARY KEY(delivery,binding)
);
CREATE INDEX IF NOT EXISTS pv_public_versions ON pv_published(artifact,version,workspace);
CREATE INDEX IF NOT EXISTS pv_decision_changes ON pv_point(task,json_extract(event,'$.decision_receipt'));
