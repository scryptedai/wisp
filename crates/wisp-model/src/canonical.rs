//! Canonical CBOR (RFC 8949 §4.2.1) for targets, inputs, lineage, and the changeset digest (§10.3).
//!
//! Definite lengths, shortest integer encodings, map keys sorted by their encoded bytes,
//! absent optional fields omitted rather than encoded as null.

use crate::ids::*;
use crate::ops::*;
use sha2::{Digest, Sha256};

/// A small CBOR value model sufficient for wisp's metadata.
#[derive(Clone, Debug, PartialEq)]
pub enum Cbor {
    Uint(u64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Bool(bool),
}

fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    if n < 24 {
        out.push(m | n as u8);
    } else if n <= 0xff {
        out.push(m | 24);
        out.push(n as u8);
    } else if n <= 0xffff {
        out.push(m | 25);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else if n <= 0xffff_ffff {
        out.push(m | 26);
        out.extend_from_slice(&(n as u32).to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

impl Cbor {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Cbor::Uint(n) => head(out, 0, *n),
            Cbor::Bytes(b) => {
                head(out, 2, b.len() as u64);
                out.extend_from_slice(b);
            }
            Cbor::Text(s) => {
                head(out, 3, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            Cbor::Array(items) => {
                head(out, 4, items.len() as u64);
                for i in items {
                    i.encode_into(out);
                }
            }
            Cbor::Map(entries) => {
                let mut enc: Vec<(Vec<u8>, Vec<u8>)> = entries
                    .iter()
                    .map(|(k, v)| (k.encode(), v.encode()))
                    .collect();
                enc.sort_by(|a, b| a.0.cmp(&b.0));
                head(out, 5, enc.len() as u64);
                for (k, v) in enc {
                    out.extend_from_slice(&k);
                    out.extend_from_slice(&v);
                }
            }
            Cbor::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
        }
    }

    pub fn map() -> MapBuilder {
        MapBuilder(Vec::new())
    }
    pub fn text(s: impl Into<String>) -> Cbor {
        Cbor::Text(s.into())
    }
    pub fn bytes(b: impl AsRef<[u8]>) -> Cbor {
        Cbor::Bytes(b.as_ref().to_vec())
    }
}

pub struct MapBuilder(Vec<(Cbor, Cbor)>);

impl MapBuilder {
    pub fn put(mut self, k: &str, v: Cbor) -> Self {
        self.0.push((Cbor::text(k), v));
        self
    }
    pub fn opt(self, k: &str, v: Option<Cbor>) -> Self {
        match v {
            Some(v) => self.put(k, v),
            None => self,
        }
    }
    pub fn build(self) -> Cbor {
        Cbor::Map(self.0)
    }
}

// ---- Encoders for wisp types ----

pub fn payload_ref(p: &PayloadRef) -> Cbor {
    Cbor::map()
        .put("id", Cbor::bytes(p.id.bytes()))
        .put("commit", Cbor::bytes(p.commit.0))
        .build()
}

pub fn target(t: &Target) -> Cbor {
    match t {
        Target::Concept { concept } => Cbor::map()
            .put("kind", Cbor::text("concept"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .build(),
        Target::Section { concept, section } => Cbor::map()
            .put("kind", Cbor::text("section"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .put("section", Cbor::bytes(section.bytes()))
            .build(),
        Target::FrontmatterKey {
            concept,
            key_commit,
        } => Cbor::map()
            .put("kind", Cbor::text("frontmatter_key"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .put("key_commit", Cbor::bytes(key_commit.0))
            .build(),
        Target::Source { concept, source } => Cbor::map()
            .put("kind", Cbor::text("source"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .put("source", Cbor::text(source.clone()))
            .build(),
        Target::Asset { asset } => Cbor::map()
            .put("kind", Cbor::text("asset"))
            .put("asset", Cbor::bytes(asset.bytes()))
            .build(),
        Target::Actor { actor } => Cbor::map()
            .put("kind", Cbor::text("actor"))
            .put("actor", Cbor::bytes(actor.bytes()))
            .build(),
        Target::Group { group } => Cbor::map()
            .put("kind", Cbor::text("group"))
            .put("group", Cbor::bytes(group.bytes()))
            .build(),
        Target::Erase { scopes } => Cbor::map()
            .put("kind", Cbor::text("erase"))
            .put(
                "scopes",
                Cbor::Array(scopes.iter().map(|s| Cbor::bytes(s.bytes())).collect()),
            )
            .build(),
    }
}

pub fn input_ref(i: &InputRef) -> Cbor {
    match i {
        InputRef::Concept {
            concept,
            section,
            at,
        } => Cbor::map()
            .put("kind", Cbor::text("concept"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .opt("section", section.map(|s| Cbor::bytes(s.bytes())))
            .opt("at", at.map(|a| Cbor::bytes(a.bytes())))
            .build(),
        InputRef::Source { concept, source } => Cbor::map()
            .put("kind", Cbor::text("source"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .put("source", Cbor::text(source.clone()))
            .build(),
        InputRef::Asset { asset, at } => Cbor::map()
            .put("kind", Cbor::text("asset"))
            .put("asset", Cbor::bytes(asset.bytes()))
            .opt("at", at.map(|a| Cbor::bytes(a.bytes())))
            .build(),
    }
}

pub fn lineage(l: &Lineage) -> Cbor {
    match l {
        Lineage::SplitFrom { section } => Cbor::map()
            .put("kind", Cbor::text("split_from"))
            .put("section", Cbor::bytes(section.bytes()))
            .build(),
        Lineage::MergedFrom { sections } => Cbor::map()
            .put("kind", Cbor::text("merged_from"))
            .put(
                "sections",
                Cbor::Array(sections.iter().map(|s| Cbor::bytes(s.bytes())).collect()),
            )
            .build(),
        Lineage::MovedFrom { concept } => Cbor::map()
            .put("kind", Cbor::text("moved_from"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .build(),
        Lineage::CopiedFrom { concept, section } => Cbor::map()
            .put("kind", Cbor::text("copied_from"))
            .put("concept", Cbor::bytes(concept.bytes()))
            .put("section", Cbor::bytes(section.bytes()))
            .build(),
    }
}

pub fn interval(v: &Interval) -> Cbor {
    Cbor::map()
        .opt("from", v.from.clone().map(Cbor::text))
        .opt("to", v.to.clone().map(Cbor::text))
        .build()
}

pub fn origin(o: &Origin) -> Cbor {
    Cbor::map()
        .put("actor_id", Cbor::bytes(o.actor_id.bytes()))
        .put("at", Cbor::text(o.at.clone()))
        .build()
}

/// `CanonicalChangeMetadata` (§10.3): IDs and keyed commitments only, never plaintext.
pub fn change_metadata(c: &Change) -> Cbor {
    Cbor::map()
        .put("id", Cbor::bytes(c.id.bytes()))
        .put("ordinal", Cbor::Uint(c.ordinal as u64))
        .put("op", Cbor::text(c.op.as_str()))
        .put("target", target(&c.target))
        .put("scope", Cbor::bytes(c.scope.bytes()))
        .opt("payload", c.payload.as_ref().map(payload_ref))
        .opt("note", c.note.as_ref().map(payload_ref))
        .put(
            "inputs",
            Cbor::Array(c.inputs.iter().map(input_ref).collect()),
        )
        .opt("valid", c.valid.as_ref().map(interval))
        .opt("lineage", c.lineage.as_ref().map(lineage))
        .opt("origin", c.origin.as_ref().map(origin))
        .opt("reason", c.reason.map(|r| Cbor::text(r.as_str())))
        .opt(
            "inputs_complete",
            c.inputs_complete.then_some(Cbor::Bool(true)),
        )
        .build()
}

/// The changeset digest (§10.3). `parent_digests` are the digests of each parent, in parent order.
pub fn changeset_digest(
    id: ChangesetId,
    actor_id: ActorId,
    at: &str,
    parent_digests: &[[u8; 32]],
    based_on: Option<ChangesetId>,
    message: Option<&PayloadRef>,
    changes: &[Change],
) -> [u8; 32] {
    let mut sorted: Vec<&Change> = changes.iter().collect();
    sorted.sort_by_key(|c| c.ordinal);
    let v = Cbor::map()
        .put("id", Cbor::bytes(id.bytes()))
        .put("actor_id", Cbor::bytes(actor_id.bytes()))
        .put("at", Cbor::text(at))
        .put(
            "parents",
            Cbor::Array(parent_digests.iter().map(Cbor::bytes).collect()),
        )
        .opt("based_on", based_on.map(|b| Cbor::bytes(b.bytes())))
        .opt("message", message.map(payload_ref))
        .put(
            "changes",
            Cbor::Array(sorted.iter().map(|c| change_metadata(c)).collect()),
        )
        .build();
    let bytes = v.encode();
    let mut h = Sha256::new();
    h.update(&bytes);
    let out = h.finalize();
    let mut d = [0u8; 32];
    d.copy_from_slice(&out);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortest_ints_and_sorted_keys() {
        assert_eq!(Cbor::Uint(0).encode(), vec![0x00]);
        assert_eq!(Cbor::Uint(23).encode(), vec![0x17]);
        assert_eq!(Cbor::Uint(24).encode(), vec![0x18, 0x18]);
        assert_eq!(Cbor::Uint(256).encode(), vec![0x19, 0x01, 0x00]);
        let m = Cbor::map()
            .put("zz", Cbor::Uint(1))
            .put("a", Cbor::Uint(2))
            .put("bb", Cbor::Uint(3))
            .build()
            .encode();
        // keys sorted by encoded bytes: "a" (0x61 61) < "bb" (0x62 ..) < "zz"
        assert_eq!(m[0], 0xa3);
        assert_eq!(&m[1..3], &[0x61, b'a']);
    }

    #[test]
    fn digest_is_deterministic_and_order_independent() {
        let cs = ChangesetId::new();
        let actor = ActorId::new();
        let c1 = Change {
            id: ChangeId::new(),
            changeset: cs,
            ordinal: 0,
            op: Op::CreateConcept,
            target: Target::Concept {
                concept: ConceptId::new(),
            },
            scope: ScopeId::new(),
            payload: None,
            note: None,
            inputs: vec![],
            valid: None,
            lineage: None,
            origin: None,
            reason: None,
            inputs_complete: false,
        };
        let mut c2 = c1.clone();
        c2.id = ChangeId::new();
        c2.ordinal = 1;
        let a = changeset_digest(
            cs,
            actor,
            "2026-10-19T00:00:00Z",
            &[],
            None,
            None,
            &[c1.clone(), c2.clone()],
        );
        let b = changeset_digest(
            cs,
            actor,
            "2026-10-19T00:00:00Z",
            &[],
            None,
            None,
            &[c2, c1],
        );
        assert_eq!(a, b);
    }

    #[test]
    fn inputs_complete_is_omitted_when_false() {
        let c = Change {
            id: ChangeId::new(),
            changeset: ChangesetId::new(),
            ordinal: 0,
            op: Op::UpdateSection,
            target: Target::Section {
                concept: ConceptId::new(),
                section: SectionId::new(),
            },
            scope: ScopeId::new(),
            payload: None,
            note: None,
            inputs: vec![],
            valid: None,
            lineage: None,
            origin: None,
            reason: None,
            inputs_complete: false,
        };
        let plain = change_metadata(&c).encode();
        let Cbor::Map(entries) = change_metadata(&c) else {
            panic!()
        };
        assert!(entries
            .iter()
            .all(|(k, _)| k != &Cbor::text("inputs_complete")));
        let mut flagged = c.clone();
        flagged.inputs_complete = true;
        assert_ne!(change_metadata(&flagged).encode(), plain);
        // The pre-flag encoding is unchanged: a record without the key decodes to false and
        // re-encodes to the same bytes.
        let json = serde_json::to_value(&c).unwrap();
        assert!(json.get("inputs_complete").is_none());
        let back: Change = serde_json::from_value(json).unwrap();
        assert_eq!(change_metadata(&back).encode(), plain);
    }
}
