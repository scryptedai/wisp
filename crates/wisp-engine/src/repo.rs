//! Repository handle: manifest, ledger, keystore, hot tiers, payload helpers.

use crate::manifest::Manifest;
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
}

impl<'a> Reader<'a> {
    pub fn read_payload(&self, scope: ScopeId, r: &PayloadRef) -> Result<Option<Vec<u8>>> {
        let Some(key) = self.keystore.get(scope)? else {
            return Ok(None);
        };
        let Some(bytes) = Ledger::read_payload_at(self.conn, &self.payloads_dir, r.id)? else {
            return Ok(None);
        };
        let (kind, pt) =
            wisp_crypto::decrypt_payload(&key.enc_key(self.repo_id), r.id, scope, &bytes)?;
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
        let master = Self::master_key(&wisp_dir, unlock, true)?;
        let keystore = Keystore::open(&wisp_dir.join("keys/keystore.db"), master)?;
        let repo_scope = ScopeId::new();
        keystore.put(repo_scope, &ScopeKey::generate())?;
        let mut manifest = Manifest::new(repo_id, profile.id());
        manifest.repo.scope = Some(repo_scope);
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
        let repo = Repository {
            root: root.to_path_buf(),
            wisp_dir,
            manifest,
            ledger,
            keystore,
            profile,
            hots: HashMap::new(),
        };
        Ok(repo)
    }

    pub fn open(root: &Path, unlock: &Unlock) -> Result<Self> {
        let wisp_dir = root.join(WISP_DIR);
        if !wisp_dir.is_dir() {
            return Err(WispError::not_found(format!(
                "no .wisp/ in {}",
                root.display()
            )));
        }
        let manifest = Manifest::load(&wisp_dir.join("manifest.toml"))?;
        let profile = wisp_profile::by_id(&manifest.repo.profile).ok_or_else(|| {
            WispError::usage(format!(
                "manifest names unknown profile {}",
                manifest.repo.profile
            ))
        })?;
        let master = Self::master_key(&wisp_dir, unlock, false)?;
        let keystore = Keystore::open(&wisp_dir.join("keys/keystore.db"), master)?;
        let ledger = Ledger::open(&wisp_dir, manifest.commit.busy_timeout_ms)?;
        let mut repo = Repository {
            root: root.to_path_buf(),
            wisp_dir,
            manifest,
            ledger,
            keystore,
            profile,
            hots: HashMap::new(),
        };
        // Recovery, before any other operation (§10.2 step 5, §10.5 Recovery).
        repo.ledger.flush_staged()?;
        crate::erase::recover(&mut repo)?;
        Ok(repo)
    }

    pub fn repo_id(&self) -> RepoId {
        self.manifest.repo.id
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
        let ct = wisp_crypto::encrypt_payload(&key.enc_key(repo), id, scope, kind, plaintext);
        Ok(PreparedPayload {
            reference: PayloadRef { id, commit },
            scope,
            kind,
            size: plaintext.len(),
            ciphertext: Some(ct),
        })
    }

    /// A payload reader bound to this repository's ledger connection.
    pub fn reader(&self) -> Reader<'_> {
        Reader {
            conn: self.ledger.conn(),
            payloads_dir: self.ledger.payloads_dir(),
            keystore: &self.keystore,
            repo_id: self.repo_id(),
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

    pub fn head(&self, ref_name: &str) -> Result<Option<ChangesetId>> {
        Ledger::head(self.ledger.conn(), ref_name)
    }

    pub fn now(&self) -> String {
        now_rfc3339()
    }
}
