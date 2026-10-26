//! `wisp-match`: section identity on record (§7.5) and concept identity on record (§7.4).
//!
//! Identity is deterministic: no similarity scoring. Every decision names the rule that made it,
//! which `wisp diff --explain` prints.

use std::collections::{HashMap, HashSet};
use wisp_model::{ConceptId, SectionId};
use wisp_parse::{Anchor, ParsedSection};

/// A section as the target ref's hot tier knows it.
#[derive(Clone, Debug)]
pub struct PriorSection {
    pub id: SectionId,
    pub level: u8,
    pub heading: String,
    pub heading_line: String,
    pub body: String,
}

/// What the matcher needs to know about the rest of the repository.
pub trait Context {
    /// The concept that currently owns `section`, if it is a known section of another concept.
    fn owner_of(&self, section: SectionId) -> Option<ConceptId>;
    /// Whether that owner still contains the section in the files being recorded now.
    fn owner_still_has(&self, section: SectionId) -> bool;
    /// Whether this ID belongs to an erased scope (never reused, §7.5 rule 4).
    fn is_erased(&self, section: SectionId) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Rule 1: known anchor, same concept.
    Same { id: SectionId },
    /// Rule 2: known anchor of another concept whose owner no longer has it.
    Moved { id: SectionId, from: ConceptId },
    /// Rule 2/3: a copy; new ID with CopiedFrom lineage.
    Copied {
        new_id: SectionId,
        from_concept: ConceptId,
        from_section: SectionId,
    },
    /// Rule 4: anchor of an erased scope; new ID.
    Reminted {
        new_id: SectionId,
        erased: SectionId,
    },
    /// Rule 5: unknown well-formed anchor, adopted as the ID.
    Adopted { id: SectionId },
    /// Rule 6a: anchor repaired by heading/level or exact body.
    Repaired { id: SectionId },
    /// Rule 6b: split from the preceding anchored section.
    Split { new_id: SectionId, from: SectionId },
    /// Rule 6c: a new section.
    Added { new_id: SectionId },
}

impl Decision {
    pub fn id(&self) -> SectionId {
        match self {
            Decision::Same { id }
            | Decision::Moved { id, .. }
            | Decision::Adopted { id }
            | Decision::Repaired { id } => *id,
            Decision::Copied { new_id, .. }
            | Decision::Reminted { new_id, .. }
            | Decision::Split { new_id, .. }
            | Decision::Added { new_id } => *new_id,
        }
    }
    pub fn rule(&self) -> &'static str {
        match self {
            Decision::Same { .. } => "7.5.1 known anchor, same concept",
            Decision::Moved { .. } => "7.5.2 known anchor, another concept: move",
            Decision::Copied { .. } => "7.5.2/3 copy: new ID, CopiedFrom",
            Decision::Reminted { .. } => "7.5.4 erased anchor: new ID",
            Decision::Adopted { .. } => "7.5.5 unknown anchor adopted",
            Decision::Repaired { .. } => "7.5.6 anchor repaired",
            Decision::Split { .. } => "7.5.6 split from preceding section",
            Decision::Added { .. } => "7.5.6 new section",
        }
    }
    /// Whether the file must be rewritten to carry this section's anchor.
    pub fn needs_anchor_write(&self) -> bool {
        !matches!(
            self,
            Decision::Same { .. } | Decision::Moved { .. } | Decision::Adopted { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    pub id: SectionId,
    /// Rule 7: the surviving section whose new body contains this section's previous body.
    pub merged_into: Option<SectionId>,
}

#[derive(Clone, Debug, Default)]
pub struct MatchResult {
    /// One decision per parsed section, in document order.
    pub decisions: Vec<Decision>,
    pub removed: Vec<Removed>,
    pub reports: Vec<String>,
}

/// Normalize for comparison: line endings to `\n`, trailing whitespace trimmed per line, and
/// trailing blank lines collapsed to one line break (§7.5).
pub fn normalize(s: &str) -> String {
    let s = s.replace("\r\n", "\n");
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let (content, nl) = match line.strip_suffix('\n') {
            Some(c) => (c, "\n"),
            None => (line, ""),
        };
        out.push_str(content.trim_end());
        out.push_str(nl);
    }
    let trimmed = out.trim_end_matches('\n');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// Collapse whitespace and lowercase for heading equality (search + record match).
pub fn normalize_heading(h: &str) -> String {
    h.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Resolve the identity of each parsed section of one concept.
pub fn match_sections(
    parsed: &[ParsedSection],
    prior: &[PriorSection],
    ctx: &dyn Context,
) -> MatchResult {
    let mut res = MatchResult::default();
    let prior_by_id: HashMap<SectionId, &PriorSection> = prior.iter().map(|p| (p.id, p)).collect();
    let mut decisions: Vec<Option<Decision>> = vec![None; parsed.len()];
    let mut seen_in_file: HashSet<SectionId> = HashSet::new();
    let mut matched_prior: HashSet<SectionId> = HashSet::new();

    // Pass 1: anchored sections.
    for (i, s) in parsed.iter().enumerate() {
        let Some(anchor) = &s.anchor else {
            // The preamble is identified by position: there is exactly one per concept.
            if s.level == 0 {
                if let Some(p) = prior
                    .iter()
                    .find(|p| p.level == 0 && !matched_prior.contains(&p.id))
                {
                    matched_prior.insert(p.id);
                    decisions[i] = Some(Decision::Repaired { id: p.id });
                }
            }
            continue;
        };
        let id = match anchor {
            Anchor::Valid(id) => *id,
            Anchor::Malformed(m) => {
                res.reports
                    .push(format!("section {i}: malformed anchor {m:?} ignored"));
                continue;
            }
        };
        // Rule 3: duplicate within the file.
        if seen_in_file.contains(&id) {
            let from_concept = prior_by_id
                .get(&id)
                .map(|_| None)
                .unwrap_or_else(|| ctx.owner_of(id));
            let d = Decision::Copied {
                new_id: SectionId::new(),
                from_concept: from_concept.unwrap_or_else(|| ConceptId::from_bytes([0u8; 16])),
                from_section: id,
            };
            res.reports.push(format!(
                "section {i}: duplicate anchor {id} treated as copy"
            ));
            decisions[i] = Some(d);
            continue;
        }
        seen_in_file.insert(id);
        // Rule 4: erased.
        if ctx.is_erased(id) {
            res.reports.push(format!(
                "section {i}: anchor {id} belongs to an erased scope; minted a new ID"
            ));
            decisions[i] = Some(Decision::Reminted {
                new_id: SectionId::new(),
                erased: id,
            });
            continue;
        }
        // Rule 1: same concept.
        if prior_by_id.contains_key(&id) {
            matched_prior.insert(id);
            decisions[i] = Some(Decision::Same { id });
            continue;
        }
        // Rule 2: another concept.
        if let Some(owner) = ctx.owner_of(id) {
            if ctx.owner_still_has(id) {
                res.reports.push(format!(
                    "section {i}: anchor {id} is owned by another concept that still has it; copied"
                ));
                decisions[i] = Some(Decision::Copied {
                    new_id: SectionId::new(),
                    from_concept: owner,
                    from_section: id,
                });
            } else {
                decisions[i] = Some(Decision::Moved { id, from: owner });
            }
            continue;
        }
        // Rule 5: unknown, well-formed.
        decisions[i] = Some(Decision::Adopted { id });
    }

    // Pass 2: unanchored sections, in order.
    let mut missing: Vec<&PriorSection> = prior
        .iter()
        .filter(|p| !matched_prior.contains(&p.id))
        .collect();
    for i in 0..parsed.len() {
        if decisions[i].is_some() {
            continue;
        }
        let s = &parsed[i];
        // Repair.
        if missing.len() == 1 {
            let m = missing[0];
            let heading_match = m.level == s.level
                && normalize_heading(&m.heading) == normalize_heading(&s.heading);
            let body_match = m.body == s.body;
            if heading_match || body_match {
                res.reports
                    .push(format!("section {}: anchor_repaired to {}", i, m.id));
                decisions[i] = Some(Decision::Repaired { id: m.id });
                matched_prior.insert(m.id);
                missing.clear();
                continue;
            }
        }
        // Split.
        if i > 0 {
            if let Some(prev) = decisions[i - 1].as_ref() {
                if let Some(p_prior) = prior_by_id.get(&prev.id()) {
                    // A heading inserted into P's body: the old body is P's new body followed by this
                    // section's body, with or without the new heading line (DECISIONS.md §7.5).
                    let with_heading = normalize(&format!(
                        "{}{}{}",
                        parsed[i - 1].body,
                        s.heading_line,
                        s.body
                    ));
                    let without_heading = normalize(&format!("{}{}", parsed[i - 1].body, s.body));
                    let old = normalize(&p_prior.body);
                    if !old.is_empty() && (old == with_heading || old == without_heading) {
                        decisions[i] = Some(Decision::Split {
                            new_id: SectionId::new(),
                            from: p_prior.id,
                        });
                        continue;
                    }
                }
            }
        }
        // Add.
        decisions[i] = Some(Decision::Added {
            new_id: SectionId::new(),
        });
    }

    // Rule 7: missing sections.
    let survivors: Vec<(SectionId, String)> = parsed
        .iter()
        .zip(decisions.iter())
        .filter_map(|(s, d)| d.as_ref().map(|d| (d.id(), normalize(&s.body))))
        .collect();
    for m in prior.iter().filter(|p| !matched_prior.contains(&p.id)) {
        let old = normalize(&m.body);
        let merged_into = if old.trim().is_empty() {
            None
        } else {
            survivors
                .iter()
                .find(|(id, body)| *id != m.id && body.contains(&old))
                .map(|(id, _)| *id)
        };
        res.removed.push(Removed {
            id: m.id,
            merged_into,
        });
    }

    res.decisions = decisions
        .into_iter()
        .map(|d| d.expect("every section decided"))
        .collect();
    res
}

/// Concept identity on record (§7.4). `known` maps wisp_id → current path for known concepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConceptDecision {
    /// A known concept at its current path, or at a new path (a move).
    Known {
        id: ConceptId,
        moved_from: Option<String>,
    },
    /// A second file carrying an ID already claimed by the file at the concept's current path.
    Copy { of: ConceptId, new_id: ConceptId },
    /// No `wisp_id`, or an unknown one: a new concept.
    New { id: ConceptId },
}

pub fn match_concepts(
    files: &[(String, Option<ConceptId>)], // (concept path, wisp_id)
    known: &HashMap<ConceptId, String>,
) -> Vec<ConceptDecision> {
    let mut out = Vec::with_capacity(files.len());
    // Which IDs appear in more than one file, and which file sits at the concept's current path.
    let mut claims: HashMap<ConceptId, Vec<usize>> = HashMap::new();
    for (i, (_, id)) in files.iter().enumerate() {
        if let Some(id) = id {
            claims.entry(*id).or_default().push(i);
        }
    }
    for (i, (path, id)) in files.iter().enumerate() {
        match id {
            Some(id) if known.contains_key(id) => {
                let current = &known[id];
                let claimants = &claims[id];
                if claimants.len() > 1 {
                    // The file at the current path keeps the ID; others are copies. If none is at the
                    // current path, the first in order keeps it.
                    let keeper = claimants
                        .iter()
                        .find(|&&j| &files[j].0 == current)
                        .copied()
                        .unwrap_or(claimants[0]);
                    if keeper != i {
                        out.push(ConceptDecision::Copy {
                            of: *id,
                            new_id: ConceptId::new(),
                        });
                        continue;
                    }
                }
                let moved_from = if path != current {
                    Some(current.clone())
                } else {
                    None
                };
                out.push(ConceptDecision::Known {
                    id: *id,
                    moved_from,
                });
            }
            Some(id) => {
                // Unknown wisp_id: adopt it as a new concept's ID unless two files claim it.
                let claimants = &claims[id];
                if claimants[0] == i {
                    out.push(ConceptDecision::New { id: *id });
                } else {
                    out.push(ConceptDecision::New {
                        id: ConceptId::new(),
                    });
                }
            }
            None => out.push(ConceptDecision::New {
                id: ConceptId::new(),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wisp_parse::sections::section;

    struct NoCtx;
    impl Context for NoCtx {
        fn owner_of(&self, _: SectionId) -> Option<ConceptId> {
            None
        }
        fn owner_still_has(&self, _: SectionId) -> bool {
            false
        }
        fn is_erased(&self, _: SectionId) -> bool {
            false
        }
    }

    fn prior_from(body: &str) -> (Vec<PriorSection>, Vec<SectionId>) {
        let s = section(body);
        let mut out = Vec::new();
        let mut ids = Vec::new();
        for p in s.sections {
            let id = match p.anchor {
                Some(Anchor::Valid(id)) => id,
                _ => SectionId::new(),
            };
            ids.push(id);
            out.push(PriorSection {
                id,
                level: p.level,
                heading: p.heading,
                heading_line: p.heading_line,
                body: p.body,
            });
        }
        (out, ids)
    }

    #[test]
    fn same_anchor_survives_rewrite() {
        let id = SectionId::new();
        let (prior, _) = prior_from(&format!("## A <!-- ^{id} -->\nold body\n"));
        let parsed = section(&format!(
            "## Completely New Heading <!-- ^{id} -->\nnew body entirely\n"
        ));
        let r = match_sections(&parsed.sections, &prior, &NoCtx);
        assert_eq!(r.decisions[1], Decision::Same { id });
        assert!(r.removed.is_empty());
    }

    #[test]
    fn repair_by_heading_and_by_body() {
        let id = SectionId::new();
        let (prior, _) = prior_from(&format!("## Schema <!-- ^{id} -->\ncolumns\n"));
        let parsed = section("## Schema\ncolumns changed\n");
        let r = match_sections(&parsed.sections, &prior, &NoCtx);
        assert_eq!(r.decisions[1], Decision::Repaired { id });
        let parsed2 = section("## Renamed\ncolumns\n");
        let r2 = match_sections(&parsed2.sections, &prior, &NoCtx);
        assert_eq!(r2.decisions[1], Decision::Repaired { id });
    }

    #[test]
    fn split_and_merge_detection() {
        let id = SectionId::new();
        let (prior, _) = prior_from(&format!(
            "## A <!-- ^{id} -->\nfirst part\n\n### B\nsecond part\n"
        ));
        // prior has A (with body containing "### B" heading? No: B is its own section). Build prior manually:
        let a = PriorSection {
            id,
            level: 2,
            heading: "A".into(),
            heading_line: "## A\n".into(),
            body: "first part\n\n### B\nsecond part\n".into(),
        };
        let prior = vec![prior[0].clone(), a];
        let parsed = section(&format!(
            "## A <!-- ^{id} -->\nfirst part\n\n### B\nsecond part\n"
        ));
        let r = match_sections(&parsed.sections, &prior, &NoCtx);
        assert_eq!(r.decisions[1], Decision::Same { id });
        assert!(matches!(r.decisions[2], Decision::Split { from, .. } if from == id));

        // Merge: B's body now appears inside A's body and B is gone.
        let b = SectionId::new();
        let prior2 = vec![
            prior[0].clone(),
            PriorSection {
                id,
                level: 2,
                heading: "A".into(),
                heading_line: "## A\n".into(),
                body: "first part\n".into(),
            },
            PriorSection {
                id: b,
                level: 3,
                heading: "B".into(),
                heading_line: "### B\n".into(),
                body: "second part\n".into(),
            },
        ];
        let parsed2 = section(&format!("## A <!-- ^{id} -->\nfirst part\nsecond part\n"));
        let r2 = match_sections(&parsed2.sections, &prior2, &NoCtx);
        assert_eq!(
            r2.removed,
            vec![Removed {
                id: b,
                merged_into: Some(id)
            }]
        );
    }

    #[test]
    fn duplicates_erased_and_unknown() {
        struct Ctx(SectionId);
        impl Context for Ctx {
            fn owner_of(&self, _: SectionId) -> Option<ConceptId> {
                None
            }
            fn owner_still_has(&self, _: SectionId) -> bool {
                false
            }
            fn is_erased(&self, s: SectionId) -> bool {
                s == self.0
            }
        }
        let id = SectionId::new();
        let erased = SectionId::new();
        let unknown = SectionId::new();
        let (prior, _) = prior_from(&format!("## A <!-- ^{id} -->\nx\n"));
        let parsed = section(&format!(
            "## A <!-- ^{id} -->\nx\n## A2 <!-- ^{id} -->\nx\n## E <!-- ^{erased} -->\ny\n## U <!-- ^{unknown} -->\nz\n"
        ));
        let r = match_sections(&parsed.sections, &prior, &Ctx(erased));
        assert_eq!(r.decisions[1], Decision::Same { id });
        assert!(
            matches!(r.decisions[2], Decision::Copied { from_section, .. } if from_section == id)
        );
        assert!(matches!(r.decisions[3], Decision::Reminted { erased: e, .. } if e == erased));
        assert_eq!(r.decisions[4], Decision::Adopted { id: unknown });
    }

    #[test]
    fn concept_identity() {
        let known_id = ConceptId::new();
        let mut known = HashMap::new();
        known.insert(known_id, "a/b".to_string());
        let files = vec![
            ("a/b".to_string(), Some(known_id)),
            ("a/c".to_string(), Some(known_id)),
            ("new".to_string(), None),
            ("moved".to_string(), Some(known_id)),
        ];
        let d = match_concepts(&files, &known);
        assert_eq!(
            d[0],
            ConceptDecision::Known {
                id: known_id,
                moved_from: None
            }
        );
        assert!(matches!(d[1], ConceptDecision::Copy { of, .. } if of == known_id));
        assert!(matches!(d[2], ConceptDecision::New { .. }));
        assert!(matches!(d[3], ConceptDecision::Copy { .. }));
        let files2 = vec![("moved".to_string(), Some(known_id))];
        let d2 = match_concepts(&files2, &known);
        assert_eq!(
            d2[0],
            ConceptDecision::Known {
                id: known_id,
                moved_from: Some("a/b".into())
            }
        );
    }
}
