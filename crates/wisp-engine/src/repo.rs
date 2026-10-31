//! Repository handle: manifest, ledger, keystore, hot tiers, payload helpers.

use crate::cold_flush::ColdFlushQueue;
use crate::commit::CommitOutcome;
use crate::manifest::{InitOptions, Manifest, PayloadStorage};
use crate::materialize::{MaterializeQueue, MaterializeStatus};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use wisp_crypto::{Keystore, MasterKey, ScopeKey};
use wisp_model::*;
use wisp_profile::Profile;
use wisp_store::{Hot, Ledger};

pub const WISP_DIR: &str = ".wisp";
pub const MAIN_REF: &str = "main";

/// How to unlock the keystore.
#[derive(Clone, Default)]
pub struct Unlock {
    pub passphrase: Option<String>,
}

/// A payload prepared in memory, before the commit (§10.2 step 1).
#[derive(Clone, Debug)]
pub struct PreparedPayload {
    pub reference: PayloadRef,
    pub scope: ScopeId,
    pub kind: PayloadKind,
    pub size: usize,
    /// None when an identical payload already exists in this scope (reused ID).
    pub ciphertext: Option<Vec<u8>>,
}

/// Reads and decrypts payloads through any ledger connection (a transaction included).
pub struct Reader<'a> {
    pub conn: &'a rusqlite::Connection,
    pub payloads_dir: PathBuf,
    pub keystore: &'a Keystore,
    pub repo_id: RepoId,
    pub encrypted: bool,
}

impl<'a> Reader<'a> {
    pub fn read_payload(&self, scope: ScopeId, r: &PayloadRef) -> Result<Option<Vec<u8>>> {
        let Some(key) = self.keystore.get(scope)? else {
            return Ok(None);
        };
        let Some(bytes) = Ledger::read_payload_at(self.conn, &self.payloads_dir, r.id)? else {
            return Ok(None);
        };
        let enc = if self.encrypted {
            Some(key.enc_key(self.repo_id))
        } else {
            None
        };
        let (kind, pt) =
            wisp_crypto::read_payload_file(self.encrypted, enc.as_ref(), r.id, scope, &bytes)?;
        let c = wisp_crypto::commitment(&key.commit_key(self.repo_id), kind, &pt);
        if c != r.commit {
            return Err(WispError::integrity(format!(
                "payload {} commitment mismatch",
                r.id
            )));
        }
        Ok(Some(pt))
    }
    pub fn read_payload_string(&self, scope: ScopeId, r: &PayloadRef) -> Result<Option<String>> {
        Ok(self
            .read_payload(scope, r)?
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    }
    pub fn scope_erased(&self, scope: ScopeId) -> Result<bool> {
        Ok(Ledger::scope(self.conn, scope)?
            .map(|s| s.erased)
            .unwrap_or(false))
    }
}

pub struct Repository {
    pub root: PathBuf,
    pub wisp_dir: PathBuf,
    pub manifest: Manifest,
    pub ledger: Ledger,
    pub keystore: Keystore,
    pub profile: Box<dyn Profile>,
    pub(crate) hots: HashMap<String, Hot>,
    cold_flush: ColdFlushQueue,
    materialize: Option<MaterializeQueue>,
}

impl Repository {
    /// Find the nearest ancestor containing `.wisp/`.
    pub fn discover(start: &Path) -> Option<PathBuf> {
        let mut cur = Some(start.to_path_buf());
        while let Some(p) = cur {
            if p.join(WISP_DIR).is_dir() {
                return Some(p);
            }
            cur = p.parent().map(|x| x.to_path_buf());
        }
        None
    }

    fn master_key(wisp_dir: &Path, unlock: &Unlock, create: bool) -> Result<MasterKey> {
        if let Some(k) = MasterKey::from_env()? {
            return Ok(k);
        }
        if let Some(p) = &unlock.passphrase {
            let salt = Keystore::passphrase_salt(&wisp_dir.join("keys/keystore.db"))?;
            return MasterKey::from_passphrase(p, &salt);
        }
        MasterKey::from_file(&wisp_dir.join("keys/master.key"), create)
    }

    /// `wisp init` (§16): create `.wisp/`, the repo scope, default policies, and `main`.
    pub fn init(root: &Path, profile_version: &str, unlock: &Unlock) -> Result<Self> {
        Self::init_with(root, profile_version, unlock, InitOptions::default())
    }

    /// `wisp init` with explicit cold-storage options (`--encrypt` sets `encrypt: true`).
    pub fn init_with(
        root: &Path,
        profile_version: &str,
        unlock: &Unlock,
        options: InitOptions,
    ) -> Result<Self> {
        let wisp_dir = root.join(WISP_DIR);
        if wisp_dir.exists() {
            return Err(WispError::usage(format!(
                "{} already exists",
                wisp_dir.display()
            )));
        }
        let profile = wisp_profile::by_version(profile_version).ok_or_else(|| {
            WispError::usage(format!("unknown OKF profile version {profile_version}"))
        })?;
        std::fs::create_dir_all(wisp_dir.join("keys"))?;
        std::fs::create_dir_all(wisp_dir.join("payloads"))?;
        std::fs::create_dir_all(wisp_dir.join("cache"))?;
        std::fs::create_dir_all(wisp_dir.join("worktrees"))?;
        std::fs::write(wisp_dir.join("cache/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n# This directory is a wisp cache; it is rebuilt from the ledger.\n")?;
        std::fs::write(
            wisp_dir.join("worktrees/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )?;
        let repo_id = RepoId::new();
        let storage = if options.encrypt {
            PayloadStorage::Encrypted
        } else {
            if unlock.passphrase.is_some() {
                return Err(WispError::usage(
                    "passphrase unlock requires `wisp init --encrypt`",
                ));
            }
            if MasterKey::from_env()?.is_some() {
                return Err(WispError::usage(
                    "WISP_MASTER_KEY is only for encrypted repositories; use `wisp init --encrypt`",
                ));
            }
            PayloadStorage::Plain
        };
        let keystore = if storage == PayloadStorage::Encrypted {
            let master = Self::master_key(&wisp_dir, unlock, true)?;
            Keystore::open(&wisp_dir.join("keys/keystore.db"), master)?
        } else {
            Keystore::open_plain(&wisp_dir.join("keys/keystore.db"))?
        };
        let repo_scope = ScopeId::new();
        keystore.put(repo_scope, &ScopeKey::generate())?;
        let mut manifest = Manifest::new(repo_id, profile.id());
        manifest.repo.scope = Some(repo_scope);
        manifest.repo.storage = Some(storage);
        manifest.save(&wisp_dir.join("manifest.toml"))?;
        let mut ledger = Ledger::open(&wisp_dir, manifest.commit.busy_timeout_ms)?;
        {
            let tx = ledger.begin_immediate()?;
            Ledger::insert_scope(&tx, repo_scope, &ScopeKind::Repo)?;
            tx.commit()
                .map_err(|e| WispError::internal(e.to_string()))?;
        }
        if !root.join(".wispignore").exists() {
            std::fs::write(
                root.join(".wispignore"),
                "# gitignore syntax; paths wisp never records\n.git/\n.DS_Store\n",
            )?;
        }
        let busy = manifest.commit.busy_timeout_ms;
        let tick = manifest.materialize.tick_ms;
        let repo = Repository {
            root: root.to_path_buf(),
            wisp_dir: wisp_dir.clone(),
            manifest,
            ledger,
            keystore,
            profile,
            hots: HashMap::new(),
            cold_flush: ColdFlushQueue::spawn(wisp_dir, busy),
            materialize: Some(MaterializeQueue::spawn(
                root.to_path_buf(),
                unlock.clone(),
                tick,
            )),
        };
        Ok(repo)
    }

    /// Background materialize worker: no nested drain threads (commit queue writer).
    pub fn open_worker(root: &Path, unlock: &Unlock) -> Result<Self> {
        Self::open_inner(root, unlock, true)
    }

    pub fn open(root: &Path, unlock: &Unlock) -> Result<Self> {
        Self::open_inner(root, unlock, false)
    }

    fn open_inner(root: &Path, unlock: &Unlock, worker: bool) -> Result<Self> {
        let wisp_dir = root.join(WISP_DIR);
        if !wisp_dir.is_dir() {
            return Err(WispError::not_found(format!(
                "no .wisp/ in {}",
                root.display()
            )));
        }
        let manifest = Manifest::load(&wisp_dir.join("manifest.toml"))?;
        if manifest.repo.format > crate::manifest::FORMAT_SUPPORTED {
            return Err(WispError::new(
                ExitCode::Refused,
                "this repository uses a newer format; upgrade wisp",
            )
            .with_details(serde_json::json!({
                "code": "format_too_new",
                "format": manifest.repo.format,
                "supported": crate::manifest::FORMAT_SUPPORTED,
            })));
        }
        let profile = wisp_profile::by_id(&manifest.repo.profile).ok_or_else(|| {
            WispError::usage(format!(
                "manifest names unknown profile {}",
                manifest.repo.profile
            ))
        })?;
        let storage = PayloadStorage::from_manifest_field(manifest.repo.storage);
        let keystore = if storage == PayloadStorage::Encrypted {
            let master = Self::master_key(&wisp_dir, unlock, false)?;
            Keystore::open(&wisp_dir.join("keys/keystore.db"), master)?
        } else {
            if unlock.passphrase.is_some() || MasterKey::from_env()?.is_some() {
                return Err(WispError::usage(
                    "this repository uses plain cold storage; do not pass --passphrase or WISP_MASTER_KEY",
                ));
            }
            Keystore::open_plain(&wisp_dir.join("keys/keystore.db"))?
        };
        let ledger = Ledger::open(&wisp_dir, manifest.commit.busy_timeout_ms)?;
        let busy = manifest.commit.busy_timeout_ms;
        let tick = manifest.materialize.tick_ms;
        let mut repo = Repository {
            root: root.to_path_buf(),
            wisp_dir: wisp_dir.clone(),
            manifest,
            ledger,
            keystore,
            profile,
            hots: HashMap::new(),
            cold_flush: if worker {
                ColdFlushQueue::inactive()
            } else {
                ColdFlushQueue::spawn(wisp_dir.clone(), busy)
            },
            materialize: if worker {
                None
            } else {
                Some(MaterializeQueue::spawn(
                    root.to_path_buf(),
                    unlock.clone(),
                    tick,
                ))
            },
        };
        // Recovery on the main handle only (§10.2 step 5, §10.5 Recovery).
        if !worker {
            repo.ledger.flush_staged()?;
            crate::erase::recover(&mut repo)?;
        }
        Ok(repo)
    }

    /// Async materialize backlog for this handle (empty when disabled or worker-only).
    pub fn materialize_status(&self) -> MaterializeStatus {
        self.materialize
            .as_ref()
            .map(MaterializeQueue::status)
            .unwrap_or_default()
    }

    /// Sync barrier: drain the materialize queue on this handle (`wisp materialize --wait`).
    pub fn flush_materialize(&mut self) -> Result<()> {
        if let Some(m) = self.materialize.take() {
            m.drain_blocking(self)?;
            self.materialize = Some(m);
        }
        Ok(())
    }

    /// Enqueue (or sync-publish when materialize is off) a delta from `outcome`.
    ///
    /// `reconcile == false` for headless commits (skip disk-dirty paths); `true` after `record`.
    pub fn schedule_materialize(
        &mut self,
        ref_name: &str,
        outcome: &CommitOutcome,
        reconcile: bool,
    ) -> Result<()> {
        let mut dirty =
            crate::publish::dirty_from_changes(self, ref_name, &outcome.changes, reconcile)?;
        if dirty.is_empty() {
            return Ok(());
        }
        dirty.reconcile = reconcile;
        if !self.manifest.materialize.enabled {
            if reconcile {
                return self.publish_dirty_immediate(ref_name, &dirty);
            }
            return Ok(());
        }
        let roots = crate::publish::managed_tree_roots(self, ref_name)?;
        if let Some(m) = &self.materialize {
            for root in roots {
                m.push_job(root, ref_name, dirty.clone());
            }
            m.wake();
        }
        Ok(())
    }

    fn publish_dirty_immediate(
        &mut self,
        ref_name: &str,
        dirty: &crate::publish::DirtySet,
    ) -> Result<()> {
        let roots = crate::publish::managed_tree_roots(self, ref_name)?;
        for root in roots {
            crate::checkout::publish_ref_delta(self, ref_name, &root, dirty)?;
        }
        Ok(())
    }

    /// Enqueue a full-ref render (checkout, merge replay, explicit barriers).
    pub fn schedule_materialize_full(&mut self, ref_name: &str) -> Result<()> {
        if !self.manifest.materialize.enabled {
            return Ok(());
        }
        let mut dirty = crate::publish::DirtySet::all_for_ref(self, ref_name)?;
        dirty.reconcile = true;
        let roots = crate::publish::managed_tree_roots(self, ref_name)?;
        if let Some(m) = &self.materialize {
            for root in roots {
                m.push_job(root, ref_name, dirty.clone());
            }
            m.wake();
        }
        Ok(())
    }

    /// Stage wake: drain staged payloads to disk on a background thread (§10.2 step 3).
    pub(crate) fn notify_cold_flush(&self) {
        self.cold_flush.wake();
    }

    pub(crate) fn maybe_backpressure_cold_flush(&self) -> Result<()> {
        self.cold_flush.maybe_backpressure(&self.ledger)
    }

    /// Sync barrier: block until all staged payloads are on disk (push, export, graceful shutdown).
    pub fn flush_cold_payloads(&mut self) -> Result<()> {
        self.cold_flush.drain_blocking(&self.ledger)
    }

    pub fn repo_id(&self) -> RepoId {
        self.manifest.repo.id
    }
    /// Cold storage mode from `manifest.toml` (`repo.storage`).
    pub fn payload_storage(&self) -> PayloadStorage {
        PayloadStorage::from_manifest_field(self.manifest.repo.storage)
    }
    /// Shorthand for `payload_storage() == Encrypted`.
    pub fn payloads_encrypted(&self) -> bool {
        self.payload_storage() == PayloadStorage::Encrypted
    }
    pub fn repo_scope(&self) -> ScopeId {
        self.manifest
            .repo
            .scope
            .expect("repo scope recorded at init")
    }
    pub fn identity_key(&self) -> &str {
        &self.manifest.identity.key
    }

    // ---- keys and payloads ----

    pub fn scope_key(&self, scope: ScopeId) -> Result<ScopeKey> {
        self.keystore.get(scope)?.ok_or_else(|| {
            WispError::key_unavailable(format!("no key for scope {scope} (erased or unknown)"))
        })
    }

    /// Mint a scope key and write it to the keystore in its own transaction (§10.2 step 1).
    pub fn new_scope_key(&self, scope: ScopeId) -> Result<()> {
        self.keystore.put(scope, &ScopeKey::generate())
    }

    /// Encrypt a payload under a scope. Within one scope, an identical plaintext reuses the existing payload ID.
    pub fn prepare_payload(
        &self,
        scope: ScopeId,
        kind: PayloadKind,
        plaintext: &[u8],
    ) -> Result<PreparedPayload> {
        let key = self.scope_key(scope)?;
        let repo = self.repo_id();
        let commit = wisp_crypto::commitment(&key.commit_key(repo), kind, plaintext);
        if let Some(existing) = Ledger::find_payload_by_commit(self.ledger.conn(), scope, &commit)?
        {
            return Ok(PreparedPayload {
                reference: PayloadRef {
                    id: existing,
                    commit,
                },
                scope,
                kind,
                size: plaintext.len(),
                ciphertext: None,
            });
        }
        let id = PayloadId::new();
        let on_disk = wisp_crypto::store_payload_file(
            self.payloads_encrypted(),
            &key.enc_key(repo),
            id,
            scope,
            kind,
            plaintext,
        );
        Ok(PreparedPayload {
            reference: PayloadRef { id, commit },
            scope,
            kind,
            size: plaintext.len(),
            ciphertext: Some(on_disk),
        })
    }

    /// A payload reader bound to this repository's ledger connection.
    pub fn reader(&self) -> Reader<'_> {
        Reader {
            conn: self.ledger.conn(),
            payloads_dir: self.ledger.payloads_dir(),
            keystore: &self.keystore,
            repo_id: self.repo_id(),
            encrypted: self.payloads_encrypted(),
        }
    }

    /// Decrypt a payload. Verifies the commitment. `None` when the payload or its key is gone (erased).
    pub fn read_payload(&self, scope: ScopeId, r: &PayloadRef) -> Result<Option<Vec<u8>>> {
        self.reader().read_payload(scope, r)
    }

    pub fn read_payload_string(&self, scope: ScopeId, r: &PayloadRef) -> Result<Option<String>> {
        Ok(self
            .read_payload(scope, r)?
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    // ---- actors ----

    pub fn actor_index(&self, name: &str) -> Result<[u8; 32]> {
        let k = self.scope_key(self.repo_scope())?;
        Ok(wisp_crypto::actor_name_index(
            &k.actor_index_key(self.repo_id()),
            name,
        ))
    }

    pub fn find_actor(&self, name: &str) -> Result<Option<wisp_store::ActorRow>> {
        let idx = self.actor_index(name)?;
        Ledger::actor_by_index(self.ledger.conn(), &idx)
    }

    /// The OKF actor string, or the erased form (§8).
    pub fn actor_display(&self, id: ActorId) -> String {
        let Ok(Some(row)) = Ledger::actor(self.ledger.conn(), id) else {
            return format!("process:unknown-{}", &id.to_string()[..10]);
        };
        match self.read_payload_string(row.scope, &row.name) {
            Ok(Some(name)) => name,
            _ => {
                let prefix = if row.kind == ActorKind::Human {
                    "human"
                } else {
                    "process"
                };
                format!("{}:erased-{}", prefix, &id.to_string()[..10])
            }
        }
    }

    pub fn actor_name(&self, id: ActorId) -> Option<String> {
        let row = Ledger::actor(self.ledger.conn(), id).ok()??;
        self.read_payload_string(row.scope, &row.name).ok()?
    }

    /// The default actor: `WISP_ACTOR`, then the manifest's `[config] actor`, then `human:<user>`.
    pub fn default_actor(&self) -> String {
        if let Ok(a) = std::env::var("WISP_ACTOR") {
            if !a.trim().is_empty() {
                return a;
            }
        }
        if let Some(a) = &self.manifest.config.actor {
            return a.clone();
        }
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".into());
        format!("human:{user}")
    }

    // ---- hot tiers ----

    pub fn hot_path(&self, ref_name: &str) -> PathBuf {
        self.wisp_dir
            .join("cache/refs")
            .join(wisp_store::hot::ref_id(ref_name))
            .join("hot.db")
    }

    /// Every hot tier on disk, by ref id directory.
    pub fn hot_paths(&self) -> Vec<PathBuf> {
        let dir = self.wisp_dir.join("cache/refs");
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path().join("hot.db");
                if p.exists() {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Open the hot tier for a ref, rebuilding when missing or stale (§6.3 Recovery).
    pub fn hot(&mut self, ref_name: &str) -> Result<&mut Hot> {
        if !self.hots.contains_key(ref_name) {
            let head = Ledger::head(self.ledger.conn(), ref_name)?;
            let hot = Hot::open(&self.hot_path(ref_name))?;
            self.hots.insert(ref_name.to_string(), hot);
            let needs_rebuild = {
                let hot = self.hots.get(ref_name).unwrap();
                match (Hot::meta(hot.conn())?, head) {
                    (Some(m), Some(h)) => m.head != h || m.ref_name != ref_name,
                    (None, _) => true,
                    (Some(_), None) => true,
                }
            };
            if needs_rebuild {
                crate::apply::rebuild_hot(self, ref_name)?;
            }
        }
        Ok(self.hots.get_mut(ref_name).unwrap())
    }

    /// Hot tiers currently open in this process (for erase purge, every tier on disk is opened).
    pub fn open_all_hots(&mut self) -> Result<Vec<String>> {
        let refs = Ledger::refs(self.ledger.conn())?;
        let mut names = Vec::new();
        for (name, _) in refs {
            if self.hot_path(&name).exists() || name == MAIN_REF {
                self.hot(&name)?;
                names.push(name);
            }
        }
        Ok(names)
    }

    pub fn hot_opened(&mut self, ref_name: &str) -> Option<&mut Hot> {
        self.hots.get_mut(ref_name)
    }

    pub fn drop_hot(&mut self, ref_name: &str) {
        self.hots.remove(ref_name);
    }

    /// Close every open hot SQLite file (all refs). Call before deleting anything under
    /// `.wisp/cache/` while this handle stays alive; on Windows open `hot.db` blocks removal.
    pub fn close_all_hots(&mut self) {
        self.hots.clear();
    }

    /// Drop this handle without running `Drop` on the caller's slot, so cold-tier files under
    /// `.wisp/` (e.g. `ledger.db`) can be replaced. The caller must assign a new
    /// [`Repository`] before using the reference again (see `wisp_sync::pack::apply_pack_bytes`).
    pub fn relinquish_for_cold_overwrite(repo: &mut Repository) -> (PathBuf, PathBuf) {
        repo.close_all_hots();
        let _ = repo.flush_cold_payloads();
        let _ = repo.flush_materialize();
        let root = repo.root.clone();
        let wisp_dir = repo.wisp_dir.clone();
        unsafe {
            std::ptr::drop_in_place(repo as *mut Repository);
        }
        (root, wisp_dir)
    }

    /// Raise `repo.format` in the manifest to at least `format` (never lowers it). Called before
    /// the first ledger write that needs the newer format, so older builds refuse the repository
    /// with an upgrade message instead of misreading it.
    pub fn require_format(&mut self, format: u32) -> Result<()> {
        if self.manifest.repo.format >= format {
            return Ok(());
        }
        self.manifest.repo.format = format;
        self.manifest.save(&self.wisp_dir.join("manifest.toml"))
    }

    pub fn head(&self, ref_name: &str) -> Result<Option<ChangesetId>> {
        Ledger::head(self.ledger.conn(), ref_name)
    }

    pub fn now(&self) -> String {
        now_rfc3339()
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        if let Some(mut m) = self.materialize.take() {
            let _ = m.finish(self);
        }
        let _ = self.cold_flush.shutdown(&self.ledger);
    }
}
