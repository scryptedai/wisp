//! Operations, targets, scopes, changes and changesets (§5, §8).

use crate::ids::*;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The kind of actor. Stays in plaintext for policy evaluation (§5, §19).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    Human,
    Process,
    Producer,
}

impl ActorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActorKind::Human => "human",
            ActorKind::Process => "process",
            ActorKind::Producer => "producer",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "human" => Some(ActorKind::Human),
            "process" => Some(ActorKind::Process),
            "producer" => Some(ActorKind::Producer),
            _ => None,
        }
    }
    /// Classify an OKF actor string: `human:<id>`, `process:<id>`, or `<producer>/<version>` (OKF §7).
    pub fn classify(actor: &str) -> Option<Self> {
        if let Some(rest) = actor.strip_prefix("human:") {
            return (!rest.is_empty()).then_some(ActorKind::Human);
        }
        if let Some(rest) = actor.strip_prefix("process:") {
            return (!rest.is_empty()).then_some(ActorKind::Process);
        }
        let mut parts = actor.splitn(2, '/');
        let name = parts.next().unwrap_or("");
        let ver = parts.next().unwrap_or("");
        (!name.is_empty() && !ver.is_empty() && !name.contains(':')).then_some(ActorKind::Producer)
    }
}

/// Concept lifecycle status (OKF §5.4). Absent means `stable`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Draft,
    Stable,
    Deprecated,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Stable => "stable",
            Status::Deprecated => "deprecated",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(Status::Draft),
            "stable" => Some(Status::Stable),
            "deprecated" => Some(Status::Deprecated),
            _ => None,
        }
    }
}

/// Reason codes for retract and erase (§8). Never free text.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Withdrawn,
    Superseded,
    RemovedFromTree,
    Duplicate,
    SourceWithdrawn,
    Request,
    Policy,
    PersonalData,
    Other,
}

impl Reason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Reason::Withdrawn => "withdrawn",
            Reason::Superseded => "superseded",
            Reason::RemovedFromTree => "removed_from_tree",
            Reason::Duplicate => "duplicate",
            Reason::SourceWithdrawn => "source_withdrawn",
            Reason::Request => "request",
            Reason::Policy => "policy",
            Reason::PersonalData => "personal_data",
            Reason::Other => "other",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "withdrawn" => Reason::Withdrawn,
            "superseded" => Reason::Superseded,
            "removed_from_tree" => Reason::RemovedFromTree,
            "duplicate" => Reason::Duplicate,
            "source_withdrawn" => Reason::SourceWithdrawn,
            "request" => Reason::Request,
            "policy" => Reason::Policy,
            "personal_data" => Reason::PersonalData,
            "other" => Reason::Other,
            _ => return None,
        })
    }
}

/// Every change carries exactly one operation (§8).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    CreateConcept,
    AddSection,
    UpdateSection,
    RemoveSection,
    MoveSection,
    ReorderSections,
    Split,
    SetKey,
    UnsetKey,
    SetStatus,
    Verify,
    SourcePut,
    SourceRemove,
    Move,
    CreateAsset,
    UpdateAsset,
    Retract,
    Restore,
    GroupAdd,
    GroupRemove,
    Erase,
    RegisterActor,
    ActorKey,
}

impl Op {
    pub fn as_str(&self) -> &'static str {
        match self {
            Op::CreateConcept => "create_concept",
            Op::AddSection => "add_section",
            Op::UpdateSection => "update_section",
            Op::RemoveSection => "remove_section",
            Op::MoveSection => "move_section",
            Op::ReorderSections => "reorder_sections",
            Op::Split => "split",
            Op::SetKey => "set_key",
            Op::UnsetKey => "unset_key",
            Op::SetStatus => "set_status",
            Op::Verify => "verify",
            Op::SourcePut => "source_put",
            Op::SourceRemove => "source_remove",
            Op::Move => "move",
            Op::CreateAsset => "create_asset",
            Op::UpdateAsset => "update_asset",
            Op::Retract => "retract",
            Op::Restore => "restore",
            Op::GroupAdd => "group_add",
            Op::GroupRemove => "group_remove",
            Op::Erase => "erase",
            Op::RegisterActor => "register_actor",
            Op::ActorKey => "actor_key",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "create_concept" => Op::CreateConcept,
            "add_section" => Op::AddSection,
            "update_section" => Op::UpdateSection,
            "remove_section" => Op::RemoveSection,
            "move_section" => Op::MoveSection,
            "reorder_sections" => Op::ReorderSections,
            "split" => Op::Split,
            "set_key" => Op::SetKey,
            "unset_key" => Op::UnsetKey,
            "set_status" => Op::SetStatus,
            "verify" => Op::Verify,
            "source_put" => Op::SourcePut,
            "source_remove" => Op::SourceRemove,
            "move" => Op::Move,
            "create_asset" => Op::CreateAsset,
            "update_asset" => Op::UpdateAsset,
            "retract" => Op::Retract,
            "restore" => Op::Restore,
            "group_add" => Op::GroupAdd,
            "group_remove" => Op::GroupRemove,
            "erase" => Op::Erase,
            "register_actor" => Op::RegisterActor,
            "actor_key" => Op::ActorKey,
            _ => return None,
        })
    }
    /// Content changes drive the `generated` managed key (§8 rendering rules).
    pub fn is_content_change(&self) -> bool {
        matches!(
            self,
            Op::CreateConcept
                | Op::AddSection
                | Op::UpdateSection
                | Op::RemoveSection
                | Op::MoveSection
                | Op::ReorderSections
                | Op::Split
                | Op::SetKey
                | Op::UnsetKey
                | Op::SourcePut
                | Op::SourceRemove
        )
    }
    /// The word used in `log.md` (§8).
    pub fn log_word(&self) -> &'static str {
        match self {
            Op::CreateConcept | Op::CreateAsset => "Creation",
            Op::Move => "Move",
            Op::SetStatus => "Deprecation",
            Op::Verify => "Verification",
            Op::Retract => "Retraction",
            Op::Restore => "Restoration",
            Op::Erase => "Erasure",
            _ => "Update",
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an operation acts on (§5).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Concept {
        concept: ConceptId,
    },
    Section {
        concept: ConceptId,
        section: SectionId,
    },
    FrontmatterKey {
        concept: ConceptId,
        key_commit: Commit,
    },
    Source {
        concept: ConceptId,
        source: String,
    },
    Asset {
        asset: AssetId,
    },
    Actor {
        actor: ActorId,
    },
    Group {
        group: GroupId,
    },
    Erase {
        scopes: Vec<ScopeId>,
    },
}

impl Target {
    pub fn concept(&self) -> Option<ConceptId> {
        match self {
            Target::Concept { concept }
            | Target::Section { concept, .. }
            | Target::FrontmatterKey { concept, .. }
            | Target::Source { concept, .. } => Some(*concept),
            _ => None,
        }
    }
    pub fn section(&self) -> Option<SectionId> {
        match self {
            Target::Section { section, .. } => Some(*section),
            _ => None,
        }
    }
}

/// The kind of a scope (§10.4).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeKind {
    Repo,
    Concept {
        concept: ConceptId,
    },
    Section {
        concept: ConceptId,
        section: SectionId,
    },
    Asset {
        asset: AssetId,
    },
    Actor {
        actor: ActorId,
    },
}

impl ScopeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScopeKind::Repo => "repo",
            ScopeKind::Concept { .. } => "concept",
            ScopeKind::Section { .. } => "section",
            ScopeKind::Asset { .. } => "asset",
            ScopeKind::Actor { .. } => "actor",
        }
    }
}

/// Section lineage (§5).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Lineage {
    SplitFrom {
        section: SectionId,
    },
    MergedFrom {
        sections: Vec<SectionId>,
    },
    MovedFrom {
        concept: ConceptId,
    },
    CopiedFrom {
        concept: ConceptId,
        section: SectionId,
    },
}

/// Asserted original author, for import and on-behalf writes (§5).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Origin {
    pub actor_id: ActorId,
    pub at: String,
}

/// A derivation input (§5).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputRef {
    Concept {
        concept: ConceptId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<SectionId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<ChangesetId>,
    },
    Source {
        concept: ConceptId,
        source: String,
    },
    Asset {
        asset: AssetId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<ChangesetId>,
    },
}

/// Optional valid-time interval (§5). Stored, not indexed in v1.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Interval {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// One operation on one target (§5).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Change {
    pub id: ChangeId,
    pub changeset: ChangesetId,
    pub ordinal: u32,
    pub op: Op,
    pub target: Target,
    pub scope: ScopeId,
    pub payload: Option<PayloadRef>,
    pub note: Option<PayloadRef>,
    pub inputs: Vec<InputRef>,
    pub valid: Option<Interval>,
    pub lineage: Option<Lineage>,
    pub origin: Option<Origin>,
    /// Reason code for retract/erase. Enum, never free text. (Additive to §5; see DECISIONS.md.)
    pub reason: Option<Reason>,
}

/// Atomic group of changes; the DAG node (§5).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Changeset {
    pub id: ChangesetId,
    pub parents: Vec<ChangesetId>,
    pub based_on: Option<ChangesetId>,
    pub actor_id: ActorId,
    pub at: String,
    pub message: Option<PayloadRef>,
    pub digest: [u8; 32],
    pub sig: Option<Vec<u8>>,
}

/// Section payload: heading level, the raw heading line(s) without the anchor, heading text, and body.
/// Rendering `heading_line + anchor + body` reproduces the source bytes (§7.3).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SectionPayload {
    pub level: u8,
    pub heading: String,
    /// The heading line(s) exactly as written, anchor removed, including the trailing line break.
    /// Empty for the preamble.
    pub heading_line: String,
    pub body: String,
}

/// Frontmatter entry payload (key name + raw span). See DECISIONS.md for why key and value share a payload.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FrontmatterPayload {
    pub key: String,
    /// The raw span bytes of the entry, including any comment lines directly above and the trailing line break.
    pub raw: String,
}

/// Source record payload (OKF `sources[]` entry) as JSON.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SourcePayload {
    pub id: String,
    pub record: serde_json::Value,
}

/// Group membership payload.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GroupMemberPayload {
    pub kind: String, // concept | section | asset
    pub member: String,
}

/// Payload kinds, carried inside the encrypted envelope (§5, §10.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PayloadKind {
    Section = 1,
    Note = 2,
    Path = 3,
    FrontmatterEntry = 4,
    SourceRecord = 5,
    AssetBytes = 6,
    Message = 7,
    GroupLabel = 8,
    ActorName = 9,
    ActorKey = 10,
    SectionOrder = 11,
    Status = 12,
    GroupMember = 13,
}

impl PayloadKind {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => PayloadKind::Section,
            2 => PayloadKind::Note,
            3 => PayloadKind::Path,
            4 => PayloadKind::FrontmatterEntry,
            5 => PayloadKind::SourceRecord,
            6 => PayloadKind::AssetBytes,
            7 => PayloadKind::Message,
            8 => PayloadKind::GroupLabel,
            9 => PayloadKind::ActorName,
            10 => PayloadKind::ActorKey,
            11 => PayloadKind::SectionOrder,
            12 => PayloadKind::Status,
            13 => PayloadKind::GroupMember,
            _ => return None,
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            PayloadKind::Section => "section",
            PayloadKind::Note => "note",
            PayloadKind::Path => "path",
            PayloadKind::FrontmatterEntry => "frontmatter_entry",
            PayloadKind::SourceRecord => "source_record",
            PayloadKind::AssetBytes => "asset_bytes",
            PayloadKind::Message => "message",
            PayloadKind::GroupLabel => "group_label",
            PayloadKind::ActorName => "actor_name",
            PayloadKind::ActorKey => "actor_key",
            PayloadKind::SectionOrder => "section_order",
            PayloadKind::Status => "status",
            PayloadKind::GroupMember => "group_member",
        }
    }
}
