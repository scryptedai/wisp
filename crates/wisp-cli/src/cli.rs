//! Argument parsing (§16 CLI reference).

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "wisp", version, about = "A local-first, differential knowledge store for OKF bundles", long_about = None)]
pub struct Cli {
    /// Repository directory (default: nearest ancestor containing .wisp/)
    #[arg(long, global = true)]
    pub repo: Option<PathBuf>,
    /// Actor string: <producer>/<version>, human:<id>, or process:<id> (default: WISP_ACTOR, then config)
    #[arg(long, global = true)]
    pub actor: Option<String>,
    /// Write exactly one JSON object to stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Suppress human-readable output
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Skip in-process subscribers
    #[arg(long, global = true)]
    pub no_index: bool,
    /// Unlock the keystore with a passphrase instead of the key file or WISP_MASTER_KEY
    #[arg(long, global = true)]
    pub passphrase: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn command_name(&self) -> &'static str {
        self.command.name()
    }
}

#[derive(Args, Debug, Clone, Default)]
pub struct WriteFlags {
    /// Target ref (default: the ref checked out in the working tree, else main)
    #[arg(long)]
    pub r#ref: Option<String>,
    /// Commit message (encrypted)
    #[arg(long, short = 'm')]
    pub message: Option<String>,
    /// Base version for the operation's precondition: a change ID, "absent", or "any"
    #[arg(long)]
    pub base: Option<String>,
    /// Read assertion: <path>[#section]@<version> (repeatable)
    #[arg(long = "assert")]
    pub asserts: Vec<String>,
    /// Require the ref head to be unchanged (fully serial commit)
    #[arg(long)]
    pub assert_head: Option<String>,
    /// Derivation input: <path>[#section] (repeatable)
    #[arg(long = "input")]
    pub inputs: Vec<String>,
    /// Encrypted reason text
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create .wisp/, the repo scope, default policies, and main
    Init {
        dir: Option<PathBuf>,
        #[arg(long, default_value = "0.2")]
        profile: String,
    },
    /// Adopt an existing bundle: a plain directory, or --git <repo>
    Import {
        dir: Option<PathBuf>,
        #[arg(long)]
        git: Option<PathBuf>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        r#ref: Option<String>,
        #[arg(long)]
        squash: bool,
    },
    /// Changed, new, missing, and non-conformant files; pending erasures
    Status,
    /// Section-level diff of the working tree against the ref
    Diff {
        path: Option<String>,
        #[arg(long)]
        explain: bool,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    /// Convert working-tree edits into one changeset
    Record {
        #[command(flatten)]
        w: WriteFlags,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        origin: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        explain: bool,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        allow_nonconformant: bool,
        #[arg(long)]
        adopt_generated: bool,
        #[arg(long)]
        keep_format: bool,
        #[arg(long)]
        resolve: bool,
    },
    /// Render a ref into the working tree or --to <dir>
    Checkout {
        r#ref: Option<String>,
        #[arg(long)]
        to: Option<PathBuf>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        include_retracted: bool,
        #[arg(long)]
        strip_anchors: bool,
    },
    /// Changesets on the ref, newest first
    Log {
        path: Option<String>,
        #[arg(long)]
        ops: Option<String>,
        #[arg(long)]
        actor: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        include_erased: bool,
        #[arg(long, short = 'n')]
        limit: Option<usize>,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Operations and decrypted notes of one changeset
    Show {
        changeset: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Content history, actors, origins, and the input graph
    Why {
        target: String,
        #[arg(long, default_value_t = 1)]
        depth: usize,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Raw content at a version; with --json the base version agents need
    Cat {
        target: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Record inverse operations for a changeset (cannot revert erase)
    Revert {
        changeset: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Move a concept and rewrite links to it
    Mv {
        from: String,
        to: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Set a concept's lifecycle status: draft | stable | deprecated
    Lifecycle {
        path: String,
        status: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Alias for `lifecycle <path> deprecated`
    Deprecate {
        path: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Append a verification by the actor
    Verify {
        path: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Withdraw a concept from the current view
    Retract {
        path: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Reverse a retraction
    Restore {
        path: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Destroy the keys and payloads of a section, concept, asset, group, or actor
    Erase {
        target: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long = "actor-id")]
        actor_target: Option<String>,
        #[arg(long)]
        reason: String,
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Erasure groups
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },
    /// Source records on a concept
    Source {
        #[command(subcommand)]
        cmd: SourceCmd,
    },
    /// Create a ref at the current head (or --from)
    Branch {
        name: Option<String>,
        #[arg(long)]
        from: Option<String>,
    },
    /// Rebuild the hot tier and working tree for a ref
    Switch {
        name: String,
        #[arg(long)]
        discard: bool,
    },
    /// Merge another ref (section-level three-way merge)
    Merge {
        source: String,
        #[arg(long)]
        into: Option<String>,
        #[arg(long)]
        resolutions: Option<PathBuf>,
        #[arg(long)]
        abort: bool,
    },
    /// Worktrees for long-running work
    Worktree {
        #[command(subcommand)]
        cmd: WorktreeCmd,
    },
    /// Remotes and replica sync
    Remote {
        #[command(subcommand)]
        cmd: RemoteCmd,
    },
    Push {
        remote: String,
        r#ref: Option<String>,
    },
    Pull {
        remote: String,
        r#ref: Option<String>,
    },
    /// Serve a replica over stdio
    Serve {
        #[arg(long)]
        stdio: bool,
    },
    /// Profile findings for the ref (or one path)
    Validate {
        path: Option<String>,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Migrate the bundle to another OKF version
    Migrate {
        #[arg(long)]
        to: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Export the view: --git <dir> or --archive <file>
    Export {
        #[arg(long)]
        git: Option<PathBuf>,
        #[arg(long)]
        archive: Option<PathBuf>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        r#ref: Option<String>,
        #[arg(long)]
        strip_anchors: bool,
    },
    /// Run subscribers to the head
    Index {
        #[arg(long)]
        rebuild: Option<String>,
    },
    /// Stream change events as JSON lines (never content)
    Feed {
        #[arg(long, default_value_t = 0)]
        since: i64,
    },
    /// Keyword search over current sections (vector/hybrid when an embedding codec is configured)
    Search {
        query: String,
        #[arg(long, default_value = "hybrid")]
        mode: String,
        #[arg(long, short = 'k', default_value_t = 10)]
        k: usize,
        #[arg(long)]
        codec: Option<String>,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Emit JSON for LLM consumption within a token budget
    Pack {
        paths: Vec<String>,
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long)]
        codec: Option<String>,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Token counts for a concept or section
    Tokens {
        target: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// Codecs for embeddings and tokenizers
    Codec { action: String, id: Option<String> },
    /// Integrity chain, signatures, commitments, orphaned payloads, shredded tombstones, hot tiers
    Fsck,
    /// Prune idle hot tiers, orphaned keystore entries
    Gc,
    /// Repository configuration
    Config {
        action: String,
        key: String,
        value: Option<String>,
    },
    /// Actors
    Actor {
        #[command(subcommand)]
        cmd: ActorCmd,
    },
    /// Hot tier maintenance
    Hot {
        #[command(subcommand)]
        cmd: HotCmd,
    },
    /// Consistent snapshot of manifest, ledger, and payloads (never cache or working trees)
    Backup {
        dest: PathBuf,
        #[arg(long)]
        with_keys: Option<PathBuf>,
    },
    /// Restore a snapshot into an empty directory
    RestoreBackup {
        src: PathBuf,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long)]
        into: Option<PathBuf>,
    },
    // ---- headless writes (§11.3) ----
    /// Replace a section's heading and body
    Update {
        target: String,
        #[command(flatten)]
        w: WriteFlags,
        #[arg(long)]
        heading: Option<String>,
        #[arg(long)]
        level: Option<u8>,
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        content_file: Option<PathBuf>,
        #[arg(long)]
        stdin: bool,
    },
    /// Insert a section after <key> (or ^ for the start)
    AddSection {
        path: String,
        #[command(flatten)]
        w: WriteFlags,
        #[arg(long)]
        after: Option<String>,
        #[arg(long)]
        heading: String,
        #[arg(long, default_value_t = 2)]
        level: u8,
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        content_file: Option<PathBuf>,
        #[arg(long)]
        stdin: bool,
    },
    /// Remove a section
    RemoveSection {
        target: String,
        #[command(flatten)]
        w: WriteFlags,
    },
    /// Set one top-level frontmatter entry (value is YAML/JSON)
    Set {
        path: String,
        key: String,
        value: String,
        #[command(flatten)]
        w: WriteFlags,
    },
    /// Remove one top-level frontmatter entry
    Unset {
        path: String,
        key: String,
        #[command(flatten)]
        w: WriteFlags,
    },
    /// Create a concept
    Create {
        path: String,
        #[arg(long)]
        r#type: String,
        #[arg(long)]
        frontmatter_file: Option<PathBuf>,
        #[arg(long)]
        content_file: Option<PathBuf>,
        #[arg(long)]
        content: Option<String>,
        #[command(flatten)]
        w: WriteFlags,
    },
    /// Commit a batch of operations from a file or stdin as one changeset
    Apply {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        r#ref: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum GroupCmd {
    Create {
        name: String,
    },
    List,
    Add {
        name: String,
        target: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    Remove {
        name: String,
        target: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SourceCmd {
    Add {
        path: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        resource: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        r#ref: Option<String>,
    },
    Rm {
        path: String,
        #[arg(long)]
        id: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
    Ls {
        path: String,
        #[arg(long)]
        r#ref: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum WorktreeCmd {
    Add {
        name: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        path: Option<PathBuf>,
    },
    List,
    Remove {
        name: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RemoteCmd {
    Add {
        name: String,
        url: String,
        #[arg(long)]
        trusted: bool,
    },
    List,
    Remove {
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ActorCmd {
    List,
    Key {
        actor: String,
        public_key_file: PathBuf,
        #[arg(long)]
        r#ref: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum HotCmd {
    Rebuild {
        #[arg(long)]
        r#ref: Option<String>,
    },
    Check {
        #[arg(long)]
        r#ref: Option<String>,
    },
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Init { .. } => "init",
            Command::Import { .. } => "import",
            Command::Status => "status",
            Command::Diff { .. } => "diff",
            Command::Record { .. } => "record",
            Command::Checkout { .. } => "checkout",
            Command::Log { .. } => "log",
            Command::Show { .. } => "show",
            Command::Why { .. } => "why",
            Command::Cat { .. } => "cat",
            Command::Revert { .. } => "revert",
            Command::Mv { .. } => "mv",
            Command::Lifecycle { .. } => "lifecycle",
            Command::Deprecate { .. } => "deprecate",
            Command::Verify { .. } => "verify",
            Command::Retract { .. } => "retract",
            Command::Restore { .. } => "restore",
            Command::Erase { .. } => "erase",
            Command::Group { .. } => "group",
            Command::Source { .. } => "source",
            Command::Branch { .. } => "branch",
            Command::Switch { .. } => "switch",
            Command::Merge { .. } => "merge",
            Command::Worktree { .. } => "worktree",
            Command::Remote { .. } => "remote",
            Command::Push { .. } => "push",
            Command::Pull { .. } => "pull",
            Command::Serve { .. } => "serve",
            Command::Validate { .. } => "validate",
            Command::Migrate { .. } => "migrate",
            Command::Export { .. } => "export",
            Command::Index { .. } => "index",
            Command::Feed { .. } => "feed",
            Command::Search { .. } => "search",
            Command::Pack { .. } => "pack",
            Command::Tokens { .. } => "tokens",
            Command::Codec { .. } => "codec",
            Command::Fsck => "fsck",
            Command::Gc => "gc",
            Command::Config { .. } => "config",
            Command::Actor { .. } => "actor",
            Command::Hot { .. } => "hot",
            Command::Backup { .. } => "backup",
            Command::RestoreBackup { .. } => "restore-backup",
            Command::Update { .. } => "update",
            Command::AddSection { .. } => "add-section",
            Command::RemoveSection { .. } => "remove-section",
            Command::Set { .. } => "set",
            Command::Unset { .. } => "unset",
            Command::Create { .. } => "create",
            Command::Apply { .. } => "apply",
        }
    }
}
