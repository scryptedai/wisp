//! `manifest.toml` (§15) and write policies (§9).

use serde::{Deserialize, Serialize};
use std::path::Path;
use wisp_model::*;

/// `repo.format` of a repository created by `wisp init`.
pub const FORMAT_BASE: u32 = 1;
/// `repo.format` once any change records `inputs_complete` (written the first time a change
/// uses it, never on upgrade, so repositories that never use it stay readable by older builds).
pub const FORMAT_INPUTS_COMPLETE: u32 = 2;
/// The newest `repo.format` this build reads.
pub const FORMAT_SUPPORTED: u32 = FORMAT_INPUTS_COMPLETE;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub repo: RepoSection,
    #[serde(default)]
    pub identity: IdentitySection,
    #[serde(default)]
    pub commit: CommitSection,
    #[serde(default)]
    pub materialize: MaterializeSection,
    #[serde(default)]
    pub render: RenderSection,
    #[serde(default)]
    pub config: ConfigSection,
    #[serde(default)]
    pub codecs: CodecsSection,
    #[serde(default, rename = "subscriber")]
    pub subscribers: Vec<SubscriberSection>,
    #[serde(default, rename = "remote")]
    pub remotes: Vec<RemoteSection>,
    #[serde(default, rename = "policy")]
    pub policies: Vec<Policy>,
    #[serde(default)]
    pub vector_index: VectorIndexSection,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VectorIndexSection {
    #[serde(default)]
    pub backends: std::collections::BTreeMap<String, wisp_index::VectorBackendConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoSection {
    pub id: RepoId,
    pub format: u32,
    pub profile: String,
    /// The repository scope (§10.4), by ID only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentitySection {
    pub key: String,
}
impl Default for IdentitySection {
    fn default() -> Self {
        IdentitySection {
            key: "wisp_id".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitSection {
    pub busy_timeout_ms: u64,
    pub retries: u32,
}
impl Default for CommitSection {
    fn default() -> Self {
        CommitSection {
            busy_timeout_ms: 5000,
            retries: 3,
        }
    }
}

/// Async hot → worktree materialize heartbeat (docs/WORKTREE.md).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaterializeSection {
    /// When true, commits enqueue publish work; when false, only explicit checkout/materialize run.
    pub enabled: bool,
    /// Heartbeat interval for the materialize drain thread (ms).
    pub tick_ms: u64,
}

impl Default for MaterializeSection {
    fn default() -> Self {
        MaterializeSection {
            enabled: true,
            tick_ms: 250,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RenderSection {
    pub erasure_markers: bool,
}
impl Default for RenderSection {
    fn default() -> Self {
        RenderSection {
            erasure_markers: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConfigSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodecsSection {
    #[serde(default)]
    pub embedding: Vec<String>,
    #[serde(default)]
    pub tokenizer: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_embedding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_tokenizer: Option<String>,
}
impl Default for CodecsSection {
    fn default() -> Self {
        CodecsSection {
            embedding: vec![],
            tokenizer: vec!["tok:approx:chars4".into(), "tok:approx:bytes4".into()],
            default_embedding: None,
            default_tokenizer: Some("tok:approx:chars4".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubscriberSection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteSection {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub trusted: bool,
}

/// A write policy (§9): the first matching rule decides; no match means allow.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub name: String,
    #[serde(rename = "match")]
    pub matcher: PolicyMatch,
    pub allow: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PolicyMatch {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unconditional: Option<bool>,
}

/// The facts about one operation that policies match on.
#[derive(Clone, Debug)]
pub struct PolicySubject<'a> {
    pub op: Op,
    pub concept_type: Option<&'a str>,
    pub section_heading: Option<&'a str>,
    pub key: Option<&'a str>,
    pub unconditional: bool,
}

impl<'a> Default for PolicySubject<'a> {
    fn default() -> Self {
        PolicySubject {
            op: Op::Verify,
            concept_type: None,
            section_heading: None,
            key: None,
            unconditional: false,
        }
    }
}

impl Policy {
    fn matches(&self, s: &PolicySubject<'_>) -> bool {
        let m = &self.matcher;
        if let Some(t) = &m.type_ {
            if s.concept_type != Some(t.as_str()) {
                return false;
            }
        }
        if let Some(h) = &m.section_heading {
            if s.section_heading.map(|x| x.eq_ignore_ascii_case(h)) != Some(true) {
                return false;
            }
        }
        if let Some(ops) = &m.ops {
            if !ops.iter().any(|o| o == s.op.as_str()) {
                return false;
            }
        }
        if let Some(keys) = &m.keys {
            if !s.key.map(|k| keys.iter().any(|x| x == k)).unwrap_or(false) {
                return false;
            }
        }
        if let Some(u) = m.unconditional {
            if u != s.unconditional {
                return false;
            }
        }
        true
    }
}

/// Patterns use `*` as a suffix wildcard; `human:*`, `process:*` and `*/*` also match by kind (§9).
pub fn pattern_matches(pattern: &str, actor_name: Option<&str>, kind: ActorKind) -> bool {
    match pattern {
        "human:*" if kind == ActorKind::Human => return true,
        "process:*" if kind == ActorKind::Process => return true,
        "*/*" if kind == ActorKind::Producer => return true,
        "*" => return true,
        _ => {}
    }
    let Some(name) = actor_name else { return false };
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == pattern
    }
}

/// Evaluate the policy list. Returns the denying rule's name, if any.
pub fn check_policy(
    policies: &[Policy],
    subject: &PolicySubject<'_>,
    actor_name: Option<&str>,
    kind: ActorKind,
) -> Option<String> {
    for p in policies {
        if p.matches(subject) {
            let allowed = p
                .allow
                .iter()
                .any(|pat| pattern_matches(pat, actor_name, kind));
            return if allowed { None } else { Some(p.name.clone()) };
        }
    }
    None
}

/// The default policies `wisp init` writes (§9), implementing OKF §10.3.
pub fn default_policies() -> Vec<Policy> {
    vec![
        Policy {
            name: "sanctioned-computations".into(),
            matcher: PolicyMatch {
                type_: Some("Attested Computation".into()),
                section_heading: Some("Computation".into()),
                ops: Some(vec![
                    "update_section".into(),
                    "append_section".into(),
                    "prepend_section".into(),
                    "remove_section".into(),
                    "split".into(),
                    "move_section".into(),
                ]),
                ..Default::default()
            },
            allow: vec!["human:*".into()],
        },
        Policy {
            name: "computation-contract".into(),
            matcher: PolicyMatch {
                type_: Some("Attested Computation".into()),
                keys: Some(vec![
                    "runtime".into(),
                    "parameters".into(),
                    "computation".into(),
                    "executor".into(),
                    "attester".into(),
                ]),
                ..Default::default()
            },
            allow: vec!["human:*".into()],
        },
        Policy {
            name: "verification".into(),
            matcher: PolicyMatch {
                ops: Some(vec!["verify".into()]),
                ..Default::default()
            },
            allow: vec!["human:*".into(), "process:*".into()],
        },
        Policy {
            name: "erasure".into(),
            matcher: PolicyMatch {
                ops: Some(vec!["erase".into()]),
                ..Default::default()
            },
            allow: vec!["human:*".into()],
        },
    ]
}

impl Manifest {
    pub fn new(id: RepoId, profile: &str) -> Self {
        Manifest {
            repo: RepoSection {
                id,
                format: 1,
                profile: profile.to_string(),
                scope: None,
            },
            identity: IdentitySection::default(),
            commit: CommitSection::default(),
            materialize: MaterializeSection::default(),
            render: RenderSection::default(),
            config: ConfigSection::default(),
            codecs: CodecsSection::default(),
            subscribers: vec![
                SubscriberSection {
                    name: "tokens".into(),
                    builtin: Some("tokens".into()),
                    command: None,
                },
                SubscriberSection {
                    name: "links".into(),
                    builtin: Some("links".into()),
                    command: None,
                },
                SubscriberSection {
                    name: "views".into(),
                    builtin: Some("views".into()),
                    command: None,
                },
            ],
            remotes: vec![],
            policies: default_policies(),
            vector_index: VectorIndexSection::default(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)?;
        toml::from_str(&s).map_err(|e| WispError::internal(format!("manifest: {e}")))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut s = String::from(
            "# wisp repository manifest. Plaintext; never holds user content (§15).\n",
        );
        s.push_str(
            &toml::to_string_pretty(self)
                .map_err(|e| WispError::internal(format!("manifest: {e}")))?,
        );
        std::fs::write(path, s)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_match_and_patterns() {
        let p = default_policies();
        let subj = PolicySubject {
            op: Op::Erase,
            ..Default::default()
        };
        assert_eq!(
            check_policy(&p, &subj, Some("sl-bot/0.3"), ActorKind::Producer),
            Some("erasure".into())
        );
        assert_eq!(
            check_policy(&p, &subj, Some("human:alex"), ActorKind::Human),
            None
        );
        let subj2 = PolicySubject {
            op: Op::UpdateSection,
            concept_type: Some("Attested Computation"),
            section_heading: Some("Computation"),
            ..Default::default()
        };
        assert_eq!(
            check_policy(&p, &subj2, Some("process:x"), ActorKind::Process),
            Some("sanctioned-computations".into())
        );
        let subj3 = PolicySubject {
            op: Op::UpdateSection,
            concept_type: Some("Attested Computation"),
            section_heading: Some("Notes"),
            ..Default::default()
        };
        assert_eq!(
            check_policy(&p, &subj3, Some("process:x"), ActorKind::Process),
            None
        );
        assert!(pattern_matches(
            "sl-bot/*",
            Some("sl-bot/0.3"),
            ActorKind::Producer
        ));
        // Kind patterns still apply after an actor's name is erased.
        assert!(pattern_matches("human:*", None, ActorKind::Human));
    }

    #[test]
    fn manifest_roundtrip() {
        let m = Manifest::new(RepoId::new(), "okf-0.2");
        let s = toml::to_string_pretty(&m).unwrap();
        let back: Manifest = toml::from_str(&s).unwrap();
        assert_eq!(back.policies.len(), 4);
        assert_eq!(back.identity.key, "wisp_id");
    }
}
