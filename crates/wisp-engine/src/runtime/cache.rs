//! In-memory snapshot cache (cat/pack/search keys at a published seq).

use crate::payloads::stitch_section_body;
use serde_json::{json, Value};
use std::collections::HashMap;
use wisp_model::{concept_path_of, SectionId};

#[derive(Debug, Clone)]
/// One append/prepend fragment applied to cached cat/pack payloads.
pub struct AppendPatch {
    pub concept_path: String,
    pub section: SectionId,
    pub fragment: String,
    pub prepend: bool,
}

#[derive(Debug, Clone)]
/// Cache invalidation or append patch recorded at a runtime seq bump.
pub enum PublishEvent {
    Append(AppendPatch),
    Invalidate,
}

#[derive(Debug, Clone, Default)]
/// Published read snapshot (seq, heads, cat/pack/search keys).
pub struct SnapshotCache {
    pub seq: u64,
    /// Published ref heads (updated on every successful apply).
    heads: HashMap<String, String>,
    cats: HashMap<String, Value>,
    packs: HashMap<String, PackEntry>,
    searches: HashMap<String, Value>,
    /// Per-bump publish log (runtime seq, not ledger seq).
    publish: Vec<(u64, Vec<PublishEvent>)>,
}

#[derive(Debug, Clone)]
/// Cached pack JSON at a particular runtime seq.
pub struct PackEntry {
    pub seq: u64,
    pub paths: Vec<String>,
    pub budget: Option<usize>,
    pub value: Value,
}

impl SnapshotCache {
    /// Increment and return the runtime publish seq.
    pub fn bump_seq(&mut self) -> u64 {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    /// Record the ref head after a successful apply.
    pub fn set_head(&mut self, ref_name: &str, head: &str) {
        self.heads.insert(ref_name.to_string(), head.to_string());
    }

    /// Published head for a ref, if known.
    pub fn head(&self, ref_name: &str) -> Option<String> {
        self.heads.get(ref_name).cloned()
    }

    /// Seed head from store on startup when present.
    pub fn seed_head(&mut self, ref_name: &str, head: Option<String>) {
        if let Some(h) = head {
            self.set_head(ref_name, &h);
        }
    }

    /// Append publish log entries for pack delta and invalidation.
    pub fn record_publish(&mut self, seq: u64, events: Vec<PublishEvent>) {
        if events.is_empty() {
            return;
        }
        self.publish.push((seq, events));
        const MAX: usize = 4096;
        if self.publish.len() > MAX {
            let drop = self.publish.len() - MAX;
            self.publish.drain(0..drop);
        }
    }

    /// Cache key for a cat RPC at head or pinned version.
    pub fn cat_key(ref_name: &str, path: &str, version: Option<&str>) -> String {
        match version {
            Some(v) => format!("{ref_name}:{path}@{v}"),
            None => format!("{ref_name}:{path}"),
        }
    }

    /// Cache key for a pack RPC (paths sorted for stability).
    pub fn pack_key(ref_name: &str, paths: &[String], budget: Option<usize>) -> String {
        let mut p = paths.to_vec();
        p.sort();
        format!("{ref_name}:pack:{}:{budget:?}", p.join(","))
    }

    /// Cache key for a search RPC.
    pub fn search_key(
        ref_name: &str,
        query: &str,
        mode: &str,
        k: usize,
        local_only: bool,
    ) -> String {
        format!("{ref_name}:search:{mode}:{k}:local_only={local_only}:{query}")
    }

    /// Lookup cached cat JSON.
    pub fn get_cat(&self, key: &str) -> Option<&Value> {
        self.cats.get(key)
    }

    /// Store cat JSON at the current seq.
    pub fn put_cat(&mut self, key: String, value: Value) {
        self.cats.insert(key, value);
    }

    /// Lookup cached pack entry.
    pub fn get_pack(&self, key: &str) -> Option<&PackEntry> {
        self.packs.get(key)
    }

    /// Store pack JSON and metadata at the current seq.
    pub fn put_pack(
        &mut self,
        key: String,
        seq: u64,
        paths: Vec<String>,
        budget: Option<usize>,
        value: Value,
    ) {
        self.packs.insert(
            key,
            PackEntry {
                seq,
                paths,
                budget,
                value,
            },
        );
    }

    /// Lookup cached search JSON.
    pub fn get_search(&self, key: &str) -> Option<&Value> {
        self.searches.get(key)
    }

    /// Store search JSON at the current seq.
    pub fn put_search(&mut self, key: String, value: Value) {
        self.searches.insert(key, value);
    }

    /// Drop cat/search keys touched by changed concept paths.
    pub fn invalidate_paths(&mut self, ref_name: &str, paths: &[String]) {
        let prefix = format!("{ref_name}:");
        self.cats.retain(|k, _| {
            !paths.iter().any(|p| {
                let c = concept_path_of(p);
                k.starts_with(&format!("{prefix}{c}"))
            })
        });
        self.searches
            .retain(|k, _| k.starts_with(&format!("{ref_name}:search:")));
    }

    /// Drop all pack cache entries for a ref.
    pub fn invalidate_pack_prefix(&mut self, ref_name: &str) {
        let prefix = format!("{ref_name}:pack:");
        self.packs.retain(|k, _| !k.starts_with(&prefix));
    }

    /// Apply append/prepend patches to cached cat/pack entries; returns false if a cached pack
    /// that includes the concept cannot be stitched (caller should full-invalidate).
    pub fn try_patch_appends(
        &mut self,
        ref_name: &str,
        patches: &[AppendPatch],
        new_seq: u64,
    ) -> bool {
        for patch in patches {
            patch_cat_entry(self, ref_name, patch);
        }
        for patch in patches {
            if !patch_pack_entries(self, ref_name, patch) {
                return false;
            }
        }
        for entry in self.packs.values_mut() {
            if entry.seq <= new_seq {
                entry.seq = new_seq;
            }
        }
        true
    }

    /// When `since_seq`..=`seq` contains only append patches for paths in this pack, return delta JSON.
    pub fn pack_delta_since(
        &self,
        ref_name: &str,
        paths: &[String],
        budget: Option<usize>,
        since_seq: u64,
        seq: u64,
    ) -> Option<Value> {
        if since_seq >= seq {
            return None;
        }
        let key = Self::pack_key(ref_name, paths, budget);
        let entry = self.packs.get(&key)?;
        if entry.seq != seq {
            return None;
        }
        let mut deltas = Vec::new();
        for (s, events) in &self.publish {
            if *s <= since_seq || *s > seq {
                continue;
            }
            for ev in events {
                match ev {
                    PublishEvent::Invalidate => return None,
                    PublishEvent::Append(p) => {
                        if !pack_covers_path(paths, &p.concept_path) {
                            continue;
                        }
                        deltas.push(json!({
                            "seq": s,
                            "concept": p.concept_path,
                            "section": p.section.to_string(),
                            "prepend": p.prepend,
                            "text": p.fragment,
                        }));
                    }
                }
            }
        }
        if deltas.is_empty() {
            return None;
        }
        Some(Value::Array(deltas))
    }
}

fn pack_covers_path(paths: &[String], concept_path: &str) -> bool {
    if paths.is_empty() {
        return true;
    }
    paths.iter().any(|p| concept_path_of(p) == concept_path)
}

fn patch_cat_entry(cache: &mut SnapshotCache, ref_name: &str, patch: &AppendPatch) {
    let key = SnapshotCache::cat_key(ref_name, &patch.concept_path, None);
    let Some(mut v) = cache.cats.get(&key).cloned() else {
        return;
    };
    if patch_section_in_cat_value(&mut v, patch.section, &patch.fragment, patch.prepend) {
        cache.cats.insert(key, v);
    }
    let sec_key = format!("{}:{}#{}", ref_name, patch.concept_path, patch.section);
    if let Some(mut sv) = cache.cats.get(&sec_key).cloned() {
        if let Some(obj) = sv.as_object_mut() {
            if let Some(c) = obj.get("content").and_then(|x| x.as_str()) {
                let body = stitch_section_body(c, &patch.fragment, patch.prepend);
                obj.insert("content".into(), json!(body));
                cache.cats.insert(sec_key, sv);
            }
        }
    }
}

fn patch_section_in_cat_value(
    v: &mut Value,
    section: SectionId,
    fragment: &str,
    prepend: bool,
) -> bool {
    let Some(sections) = v.get_mut("sections").and_then(|x| x.as_array_mut()) else {
        return false;
    };
    let sid = section.to_string();
    for s in sections {
        if s.get("section").and_then(|x| x.as_str()) == Some(sid.as_str()) {
            if let Some(content) = s.get("content").and_then(|x| x.as_str()) {
                let body = stitch_section_body(content, fragment, prepend);
                s["content"] = json!(body);
                return true;
            }
        }
    }
    false
}

fn patch_pack_entries(cache: &mut SnapshotCache, ref_name: &str, patch: &AppendPatch) -> bool {
    let prefix = format!("{ref_name}:pack:");
    let keys: Vec<String> = cache
        .packs
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect();
    for key in keys {
        let Some(entry) = cache.packs.get(&key) else {
            continue;
        };
        if !pack_covers_path(&entry.paths, &patch.concept_path) {
            continue;
        }
        let mut value = entry.value.clone();
        if !patch_section_in_pack_value(&mut value, patch) {
            return false;
        }
        if let Some(e) = cache.packs.get_mut(&key) {
            e.value = value;
        }
    }
    true
}

fn patch_section_in_pack_value(v: &mut Value, patch: &AppendPatch) -> bool {
    let Some(concepts) = v.get_mut("concepts").and_then(|x| x.as_array_mut()) else {
        return true;
    };
    let sid = patch.section.to_string();
    for c in concepts {
        if c.get("path").and_then(|x| x.as_str()) != Some(patch.concept_path.as_str()) {
            continue;
        }
        let Some(sections) = c.get_mut("sections").and_then(|x| x.as_array_mut()) else {
            return true;
        };
        for s in sections {
            if s.get("id").and_then(|x| x.as_str()) != Some(sid.as_str()) {
                continue;
            }
            if let Some(text) = s.get("text").and_then(|x| x.as_str()) {
                let body = stitch_section_body(text, &patch.fragment, patch.prepend);
                s["text"] = json!(body);
                return true;
            }
            return false;
        }
        // Concept in pack but section omitted from pack slice — not an error.
        return true;
    }
    true
}
