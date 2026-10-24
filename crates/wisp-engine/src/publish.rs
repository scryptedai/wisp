//! Publish set: map changesets to concept/asset paths and directory views (§9, docs/WORKTREE.md).

use crate::repo::{Repository, MAIN_REF};
use std::collections::BTreeSet;
use wisp_model::*;
use wisp_store::Hot;

/// Paths to materialize from hot into a managed worktree.
#[derive(Clone, Debug, Default)]
pub struct DirtySet {
    /// Concept paths without `.md` (e.g. `notes/foo`).
    pub concept_paths: BTreeSet<String>,
    pub asset_paths: BTreeSet<String>,
    /// Relative paths (with extension) to remove when clean on disk.
    pub prune_rels: BTreeSet<String>,
    /// Directory prefixes needing `index.md` / `log.md` (empty = bundle root).
    pub view_dirs: BTreeSet<String>,
    /// After `record` / merge replay: render even when the file differs from `checkout.idx`.
    pub reconcile: bool,
}

impl DirtySet {
    /// Recompute `view_dirs` from concept and asset path prefixes.
    pub fn finalize_views(&mut self) {
        for p in &self.concept_paths {
            let mut d = dir_of(p);
            while {
                self.view_dirs.insert(d.clone());
                !d.is_empty()
            } {
                d = dir_of(&d);
            }
        }
        for p in &self.asset_paths {
            let mut d = dir_of(p);
            while {
                self.view_dirs.insert(d.clone());
                !d.is_empty()
            } {
                d = dir_of(&d);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.concept_paths.is_empty() && self.asset_paths.is_empty() && self.prune_rels.is_empty()
    }

    pub fn merge(&mut self, other: &DirtySet) {
        self.concept_paths
            .extend(other.concept_paths.iter().cloned());
        self.asset_paths.extend(other.asset_paths.iter().cloned());
        self.prune_rels.extend(other.prune_rels.iter().cloned());
        self.view_dirs.extend(other.view_dirs.iter().cloned());
        self.reconcile = self.reconcile || other.reconcile;
    }

    /// All concepts/assets on a ref (checkout / switch / merge materialize).
    pub fn all_for_ref(repo: &mut Repository, ref_name: &str) -> Result<Self> {
        repo.hot(ref_name)?;
        let hot = repo.hots.get(ref_name).expect("hot");
        let conn = hot.conn();
        let mut dirty = DirtySet::default();
        for c in Hot::concepts(conn)? {
            dirty.concept_paths.insert(c.path.clone());
        }
        for a in Hot::assets(conn)? {
            dirty.asset_paths.insert(a.path.clone());
        }
        dirty.finalize_views();
        prune_stale_concept_paths(repo, ref_name, &mut dirty, &BTreeSet::new(), true)?;
        Ok(dirty)
    }
}

fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn concept_path(
    repo: &mut Repository,
    ref_name: &str,
    concept: ConceptId,
) -> Result<Option<String>> {
    repo.hot(ref_name)?;
    let conn = repo.hots.get(ref_name).expect("hot").conn();
    Ok(Hot::concept(conn, concept)?.map(|c| c.path))
}

/// Build a publish set from ledger changes (record, headless, merge replay).
pub fn dirty_from_changes(
    repo: &mut Repository,
    ref_name: &str,
    changes: &[Change],
    reconcile: bool,
) -> Result<DirtySet> {
    let mut dirty = DirtySet::default();
    let mut touched_concepts = BTreeSet::new();
    for ch in changes {
        if matches!(
            ch.op,
            Op::RegisterActor | Op::ActorKey | Op::GroupAdd | Op::GroupRemove
        ) {
            continue;
        }
        match &ch.target {
            Target::Concept { concept } => {
                touched_concepts.insert(*concept);
                match ch.op {
                    Op::CreateConcept | Op::Restore => {
                        if let Some(p) = concept_path(repo, ref_name, *concept)? {
                            dirty.concept_paths.insert(p);
                        }
                    }
                    Op::Move => {
                        if let Some(p) = concept_path(repo, ref_name, *concept)? {
                            dirty.concept_paths.insert(p);
                        }
                    }
                    Op::Erase => {}
                    _ => {
                        if let Some(p) = concept_path(repo, ref_name, *concept)? {
                            dirty.concept_paths.insert(p);
                        }
                    }
                }
            }
            Target::Section { concept, .. } => {
                touched_concepts.insert(*concept);
                if let Some(p) = concept_path(repo, ref_name, *concept)? {
                    dirty.concept_paths.insert(p);
                }
            }
            Target::FrontmatterKey { concept, .. } | Target::Source { concept, .. } => {
                touched_concepts.insert(*concept);
                if let Some(p) = concept_path(repo, ref_name, *concept)? {
                    dirty.concept_paths.insert(p);
                }
            }
            Target::Asset { asset } => {
                if let Some(p) = asset_path(repo, ref_name, *asset)? {
                    dirty.asset_paths.insert(p);
                }
            }
            _ => {}
        }
    }
    dirty.finalize_views();
    prune_stale_concept_paths(repo, ref_name, &mut dirty, &touched_concepts, reconcile)?;
    Ok(dirty)
}

fn prune_stale_concept_paths(
    repo: &mut Repository,
    ref_name: &str,
    dirty: &mut DirtySet,
    touched: &BTreeSet<ConceptId>,
    full_index: bool,
) -> Result<()> {
    let idx_path = crate::checkout::checkout_idx_path(repo, &repo.root);
    let idx = crate::checkout::CheckoutIndex::load(&idx_path)?;
    let Some(idx) = idx.filter(|i| i.ref_name == ref_name) else {
        return Ok(());
    };
    repo.hot(ref_name)?;
    let conn = repo.hots.get(ref_name).expect("hot").conn();
    let concepts: BTreeSet<ConceptId> = if full_index {
        idx.concepts
            .keys()
            .filter_map(|k| k.parse::<ConceptId>().ok())
            .collect()
    } else {
        touched.clone()
    };
    for concept in concepts {
        let key = concept.to_string();
        let Some(base) = idx.concepts.get(&key) else {
            continue;
        };
        let old_rel = format!("{}.md", base.path);
        match Hot::concept(conn, concept)? {
            None => {
                dirty.prune_rels.insert(old_rel);
            }
            Some(c) => {
                let new_rel = format!("{}.md", c.path);
                if old_rel != new_rel {
                    dirty.prune_rels.insert(old_rel);
                }
            }
        }
    }
    Ok(())
}

fn asset_path(repo: &mut Repository, ref_name: &str, asset: AssetId) -> Result<Option<String>> {
    repo.hot(ref_name)?;
    let conn = repo.hots.get(ref_name).expect("hot").conn();
    Ok(Hot::asset(conn, asset)?.map(|a| a.path))
}

/// Managed tree roots that should receive async materialize for `ref_name`.
pub fn managed_tree_roots(repo: &Repository, ref_name: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut roots = Vec::new();
    let primary_idx = crate::checkout::CheckoutIndex::load(&crate::checkout::checkout_idx_path(
        repo, &repo.root,
    ))?;
    match primary_idx.as_ref() {
        Some(i) if i.ref_name == ref_name => roots.push(repo.root.clone()),
        None if ref_name == MAIN_REF => roots.push(repo.root.clone()),
        _ => {}
    }
    for wt in wisp_store::Ledger::list_worktrees(repo.ledger.conn())? {
        if wt.ref_name == ref_name && std::path::Path::new(&wt.path).is_dir() {
            roots.push(std::path::PathBuf::from(wt.path));
        }
    }
    Ok(roots)
}
