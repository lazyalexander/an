//! Wire types shared by registration, decisions and both traversal directions.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ArtifactId(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(transparent)]
pub struct VersionId(pub u32);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRef {
    pub artifact: ArtifactId,
    pub version: VersionId,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockRef {
    pub artifact: ArtifactId,
    pub version: VersionId,
    pub block: String,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Quotes,
    BasedOn,
    SplitFrom,
    MergedFrom,
    Verifies,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub relation: Relation,
    pub source: BlockRef,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub original: bool,
    #[serde(default)]
    pub cites: Vec<Citation>,
    pub bytes: Option<[usize; 2]>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Material {
    Input {
        name: String,
    },
    Output {
        node: String,
        attempt: String,
        port: String,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Decision,
    Document,
    Report,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedOutcome {
    Approved,
    Suggested,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attribution {
    pub outcome: ReportedOutcome,
    pub speaker: Option<String>,
    pub source_time: Option<String>,
    pub statement: BlockRef,
    pub authority: Option<BlockRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub artifact: ArtifactId,
    pub expected: VersionId,
    pub kind: Kind,
    pub material: Option<Material>,
    pub declaration: Option<Material>,
    pub attribution: Option<Attribution>,
    pub file: Option<String>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub idempotency_key: String,
    pub versions: Vec<Draft>,
    pub reported_participant: Option<String>,
}

#[derive(Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Accept,
    Reject,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub idempotency_key: String,
    pub artifact: ArtifactId,
    pub version: VersionId,
    pub choice: Choice,
    pub seen_cursor: u64,
    pub notify_grant: Option<String>,
    pub review: Option<BlockRef>,
    pub reason: Option<String>,
    pub reported_participant: Option<String>,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
