//! Command dispatch.

use crate::cli::*;
use crate::{CliResult, Output};
use serde_json::{json, Value};
use std::path::PathBuf;
use wisp_engine::erase::EraseTarget;
use wisp_engine::headless::{ApplyBatch, ApplyOp};
use wisp_engine::query::{self, LogFilter};
use wisp_engine::record::RecordOptions;
use wisp_engine::{Repository, Unlock};
use wisp_model::*;

fn unlock(cli: &Cli) -> Unlock {
    Unlock {
        passphrase: cli.passphrase.clone(),
    }
}

fn open(cli: &Cli) -> Result<Repository> {
    let start = cli.repo.clone().unwrap_or(std::env::current_dir()?);
    let root = Repository::discover(&start).ok_or_else(|| {
        WispError::not_found(format!("no .wisp/ found at or above {}", start.display()))
    })?;
    Repository::open(&root, &unlock(cli))
}

fn actor_of(cli: &Cli, repo: &Repository) -> String {
    cli.actor.clone().unwrap_or_else(|| repo.default_actor())
}

fn ref_of(repo: &Repository, explicit: &Option<String>) -> String {
    explicit.clone().unwrap_or_else(|| query::current_ref(repo))
}

fn remote_url(repo: &Repository, name: &str) -> Result<String> {
    if let Some(r) = repo.manifest.remotes.iter().find(|r| r.name == name) {
        return Ok(r.url.clone());
    }
    let n = name.trim();
    if n.starts_with("stdio:")
        || n.starts_with("file:")
        || n.starts_with("http://")
        || n.starts_with("https://")
        || n.starts_with("ssh://")
    {
        return Ok(n.to_string());
    }
    Err(WispError::not_found(format!("remote {name:?}")))
}

fn parse_cs(s: &str) -> Result<ChangesetId> {
    s.parse()
        .map_err(|_| WispError::usage(format!("bad changeset id {s:?}")))
}

fn parse_reason(s: &str) -> Result<Reason> {
    Reason::parse(s).ok_or_else(|| WispError::usage(format!("reason must be one of withdrawn, superseded, removed_from_tree, duplicate, source_withdrawn, request, policy, personal_data, other (got {s:?})")))
}

fn read_content(
    content: Option<String>,
    file: Option<PathBuf>,
    stdin: bool,
) -> Result<Option<String>> {
    if let Some(c) = content {
        return Ok(Some(c));
    }
    if let Some(f) = file {
        return Ok(Some(std::fs::read_to_string(f)?));
    }
    if stdin {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        return Ok(Some(s));
    }
    Ok(None)
}

/// Build a one-op batch from write flags and a headless op.
fn batch_for(cli: &Cli, repo: &Repository, w: &WriteFlags, mut op: ApplyOp) -> Result<ApplyBatch> {
    op.base_version = w.base.clone();
    op.note = w.note.clone();
    let mut asserts = Vec::new();
    for a in &w.asserts {
        let (target, version) = a
            .rsplit_once('@')
            .ok_or_else(|| WispError::usage("--assert takes <path>[#section]@<version>"))?;
        let (path, section) = match target.split_once('#') {
            Some((p, s)) => (
                p.to_string(),
                Some(
                    s.parse::<SectionId>()
                        .map_err(|_| WispError::usage("bad section id in --assert"))?,
                ),
            ),
            None => (target.to_string(), None),
        };
        asserts.push(wisp_engine::headless::ApplyAssert {
            path,
            section,
            version: version
                .parse()
                .map_err(|_| WispError::usage("bad version in --assert"))?,
        });
    }
    let inputs = w
        .inputs
        .iter()
        .map(|i| wisp_engine::headless::parse_input_spec(i))
        .collect::<Result<Vec<_>>>()?;
    Ok(ApplyBatch {
        r#ref: Some(ref_of(repo, &w.r#ref)),
        base: None,
        actor: Some(actor_of(cli, repo)),
        message: w.message.clone(),
        ops: vec![op],
        inputs,
        assert: asserts,
        assert_head: w.assert_head.as_deref().map(parse_cs).transpose()?,
        inputs_complete: w.inputs_complete,
    })
}

fn empty_op(op: &str, path: &str) -> ApplyOp {
    ApplyOp {
        op: op.into(),
        path: path.into(),
        section: None,
        base_version: None,
        heading: None,
        level: None,
        content: None,
        after: None,
        key: None,
        value: None,
        r#type: None,
        frontmatter: None,
        note: None,
        split_from: None,
        merged_from: None,
        ..Default::default()
    }
}

fn split_target(target: &str) -> (String, Option<SectionId>) {
    match target.split_once('#') {
        Some((p, s)) => (p.to_string(), s.parse().ok()),
        None => (target.to_string(), None),
    }
}

fn impact_human(v: &Value) -> String {
    // ULIDs share a time prefix, so truncated ids are ambiguous: print them whole.
    let short = |x: &Value| -> String { x.as_str().unwrap_or("?").to_string() };
    let item = |path: &Value, concept: &Value, section: &Value| -> String {
        let base = match path.as_str() {
            Some(p) => format!("{p}.md"),
            None => format!("<{}>", concept.as_str().unwrap_or("?")),
        };
        match section.as_str() {
            Some(s) => format!("{base}#{s}"),
            None => base,
        }
    };
    let src = &v["source"];
    let mut out = format!(
        "Impact of {} ({}) on {} @ {} — historical, depth ≤ {}{}\n",
        item(&src["path"], &src["concept_id"], &src["section_id"]),
        src["state"].as_str().unwrap_or("?"),
        v["ref"].as_str().unwrap_or("?"),
        short(&v["head"]),
        v["depth_limit"],
        if v["truncated"] == true {
            " (truncated: dependencies beyond the depth limit were not explored)"
        } else {
            ""
        }
    );
    let results = v["results"].as_array().cloned().unwrap_or_default();
    if results.is_empty() {
        out.push_str(v["message"].as_str().unwrap_or_default());
        out.push('\n');
    } else {
        out.push_str(&format!(
            "{}:\n",
            plural(results.len(), "recorded dependent")
        ));
        for r in &results {
            out.push_str(&format!(
                "  {} @ {}  {}  {}  ({}, depth {}){}\n",
                item(&r["path"], &r["concept_id"], &r["section_id"]),
                short(&r["version"]),
                r["actor"].as_str().unwrap_or("?"),
                r["at"].as_str().unwrap_or("?"),
                r["link"].as_str().unwrap_or("?"),
                r["depth"],
                if r["merge_skipped"] == true {
                    "  [skipped by merge]"
                } else {
                    ""
                }
            ));
            for hop in r["via"].as_array().into_iter().flatten().rev() {
                let version = match hop["version"].as_str() {
                    Some(_) => format!(" @ {}", short(&hop["version"])),
                    None => String::new(),
                };
                let pin = match hop["snapshot"].as_str() {
                    Some(_) => format!(" at {}", short(&hop["snapshot"])),
                    None => String::new(),
                };
                out.push_str(&format!(
                    "      ← {}{}  [{}{}]\n",
                    item(&hop["path"], &hop["concept_id"], &hop["section_id"]),
                    version,
                    hop["link"].as_str().unwrap_or("?"),
                    pin
                ));
            }
        }
    }
    let untracked = v["untracked_content_changes"].as_u64().unwrap_or(0);
    out.push_str(&format!(
        "Recorded inputs are declared by writers and may be incomplete; {} on this ref declared none. Absence here is not evidence of independence.",
        plural(untracked as usize, "section write")
    ));
    out
}

fn status_human(s: &wisp_engine::record::Status) -> String {
    let mut out = format!(
        "On ref {} (head {})\n",
        s.ref_name,
        s.head.clone().unwrap_or_else(|| "none".into())
    );
    let section = |title: &str, items: &[String]| -> String {
        if items.is_empty() {
            String::new()
        } else {
            format!(
                "{title}:\n{}\n",
                items
                    .iter()
                    .map(|i| format!("  {i}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    };
    out.push_str(&section("Changed", &s.changed));
    out.push_str(&section("New", &s.new));
    out.push_str(&section("Missing", &s.missing));
    out.push_str(&section("Non-conformant", &s.nonconformant));
    out.push_str(&section("Generated files edited", &s.generated_modified));
    out.push_str(&section("Warnings", &s.warnings));
    if s.pending_erasures > 0 {
        out.push_str(&format!("Pending erasures: {}\n", s.pending_erasures));
    }
    if s.materialize.pending_jobs > 0 || s.materialize.pending_paths > 0 {
        out.push_str(&format!(
            "Materialize backlog: {} job(s), {} path(s)",
            s.materialize.pending_jobs, s.materialize.pending_paths
        ));
        if let Some(ms) = s.materialize.lag_ms {
            out.push_str(&format!(" (lag ~{} ms)", ms));
        }
        out.push_str(" — wisp materialize --wait or use wisp cat\n");
    }
    if s.changed.is_empty()
        && s.new.is_empty()
        && s.missing.is_empty()
        && s.nonconformant.is_empty()
        && s.generated_modified.is_empty()
    {
        out.push_str("Nothing to record.\n");
    }
    out.trim_end().to_string()
}

pub fn run(cli: Cli) -> CliResult {
    match &cli.command {
        Command::Init { dir, profile } => {
            let root = dir.clone().unwrap_or(std::env::current_dir()?);
            std::fs::create_dir_all(&root)?;
            let repo = Repository::init(&root, profile, &unlock(&cli))?;
            Ok(Output::text(
                json!({ "root": root.display().to_string(), "repo": repo.repo_id().to_string(), "profile": repo.profile.id() }),
                format!(
                    "Initialized wisp repository in {} ({})",
                    root.join(".wisp").display(),
                    repo.profile.id()
                ),
            ))
        }
        Command::Import {
            dir,
            git,
            path,
            r#ref,
            squash,
            history,
        } => {
            if let Some(git_dir) = git {
                let mut repo = open(&cli)?;
                let out = wisp_git::import_git(
                    &mut repo,
                    git_dir,
                    wisp_git::ImportGitOptions {
                        prefix: path.clone(),
                        git_ref: r#ref.clone(),
                        squash_message: if *squash {
                            Some("import --git --squash".into())
                        } else {
                            None
                        },
                        history: *history,
                    },
                )?;
                return Ok(Output::json(out));
            }
            let mut repo = open(&cli)?;
            let src = dir
                .clone()
                .ok_or_else(|| WispError::usage("import <dir> or --git <repo>"))?;
            let src = src.canonicalize()?;
            let root = repo.root.canonicalize()?;
            let mut copied = 0usize;
            if src != root {
                for e in walkdir::WalkDir::new(&src).into_iter().flatten() {
                    if e.file_type().is_file() {
                        let rel = e.path().strip_prefix(&src).unwrap();
                        if rel
                            .components()
                            .any(|c| c.as_os_str() == ".wisp" || c.as_os_str() == ".git")
                        {
                            continue;
                        }
                        let to = root.join(rel);
                        std::fs::create_dir_all(to.parent().unwrap())?;
                        std::fs::copy(e.path(), to)?;
                        copied += 1;
                    }
                }
            }
            let actor = actor_of(&cli, &repo);
            let r = wisp_engine::record::record(
                &mut repo,
                &RecordOptions {
                    actor: Some(actor),
                    message: Some(format!("Import {}", src.display())),
                    allow_nonconformant: true,
                    adopt_generated: true,
                    ..Default::default()
                },
            )?;
            Ok(Output::json(json!({ "copied": copied, "record": r })))
        }
        Command::Status => {
            let mut repo = open(&cli)?;
            let s = wisp_engine::record::status(&mut repo, None)?;
            let human = status_human(&s);
            Ok(Output::text(serde_json::to_value(&s)?, human))
        }
        Command::Materialize { wait } => {
            let mut repo = open(&cli)?;
            if *wait {
                repo.flush_materialize()?;
            }
            Ok(Output::json(json!({
                "materialize": repo.materialize_status(),
                "wait": wait,
            })))
        }
        Command::Diff {
            path,
            explain,
            from,
            to,
        } => {
            let mut repo = open(&cli)?;
            if from.is_some() || to.is_some() {
                let from = from.as_deref().ok_or_else(|| {
                    WispError::usage("diff --from and --to are required together")
                })?;
                let to = to.as_deref().ok_or_else(|| {
                    WispError::usage("diff --from and --to are required together")
                })?;
                use std::str::FromStr;
                let from_id = ChangesetId::from_str(from)
                    .map_err(|e| WispError::usage(format!("invalid --from changeset: {e}")))?;
                let to_id = ChangesetId::from_str(to)
                    .map_err(|e| WispError::usage(format!("invalid --to changeset: {e}")))?;
                let ref_name = ref_of(&repo, &None);
                let d = wisp_engine::record::diff_between_changesets(
                    &mut repo,
                    &ref_name,
                    from_id,
                    to_id,
                    path.as_deref(),
                )?;
                let _ = explain;
                return Ok(Output::json(d));
            }
            let d = wisp_engine::record::diff(&mut repo, None, *explain, path.as_deref())?;
            Ok(Output::json(d))
        }
        Command::Record {
            w,
            reason,
            origin,
            dry_run,
            explain,
            full,
            allow_nonconformant,
            adopt_generated,
            keep_format,
            resolve,
        } => {
            let mut repo = open(&cli)?;
            let actor = actor_of(&cli, &repo);
            let ref_name = ref_of(&repo, &w.r#ref);
            let specs = w
                .inputs
                .iter()
                .map(|i| wisp_engine::headless::parse_input_spec(i))
                .collect::<Result<Vec<_>>>()?;
            let inputs = wisp_engine::headless::resolve_inputs(&mut repo, &ref_name, &specs)?;
            let origin = match origin {
                Some(o) => {
                    let (a, at) = o
                        .rsplit_once('@')
                        .ok_or_else(|| WispError::usage("--origin takes <actor>@<time>"))?;
                    let row = repo.find_actor(a)?.ok_or_else(|| {
                        WispError::not_found(format!("origin actor {a} is not registered"))
                    })?;
                    Some(Origin {
                        actor_id: row.id,
                        at: at.to_string(),
                    })
                }
                None => None,
            };
            let opts = RecordOptions {
                ref_name: Some(ref_name),
                actor: Some(actor),
                message: w.message.clone(),
                reason: reason.as_deref().map(parse_reason).transpose()?,
                note: w.note.clone(),
                inputs,
                inputs_complete: w.inputs_complete,
                origin,
                dry_run: *dry_run,
                explain: *explain,
                full: *full,
                allow_nonconformant: *allow_nonconformant,
                adopt_generated: *adopt_generated,
                keep_format: *keep_format,
                resolve_merge: *resolve,
            };
            let r = wisp_engine::record::record(&mut repo, &opts)?;
            let human = match (&r.changeset, r.ops) {
                (Some(cs), n) => format!("Recorded {} as changeset {}", plural(n, "operation"), cs),
                (None, 0) => "Nothing to record.".to_string(),
                (None, n) => format!("Dry run: {} planned", plural(n, "operation")),
            };
            Ok(Output::text(serde_json::to_value(&r)?, human))
        }
        Command::Checkout {
            r#ref,
            to,
            profile,
            include_retracted,
            strip_anchors,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let idx = wisp_engine::checkout::checkout_with_profile(
                &mut repo,
                &ref_name,
                to.as_deref(),
                profile.as_deref(),
                *strip_anchors,
                *include_retracted,
            )?;
            Ok(Output::text(
                json!({ "ref": ref_name, "head": idx.head.map(|h| h.to_string()), "files": idx.files.len() }),
                format!("Checked out {} ({} files)", ref_name, idx.files.len()),
            ))
        }
        Command::Log {
            path,
            ops,
            actor,
            since,
            include_erased,
            limit,
            r#ref,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let ops = match ops {
                Some(s) => Some(
                    s.split(',')
                        .map(|o| {
                            Op::parse(o.trim())
                                .ok_or_else(|| WispError::usage(format!("unknown op {o:?}")))
                        })
                        .collect::<Result<Vec<_>>>()?,
                ),
                None => None,
            };
            let v = query::log(
                &mut repo,
                &ref_name,
                &LogFilter {
                    path: path.as_deref(),
                    ops,
                    actor: actor.as_deref(),
                    since: since.as_deref(),
                    include_erased: *include_erased,
                    limit: *limit,
                },
            )?;
            let mut human = String::new();
            for cs in v["changesets"].as_array().unwrap_or(&vec![]) {
                human.push_str(&format!(
                    "{}  {}  {}\n",
                    cs["changeset"].as_str().unwrap_or(""),
                    cs["at"].as_str().unwrap_or(""),
                    cs["actor"].as_str().unwrap_or("")
                ));
                if let Some(m) = cs["message"].as_str() {
                    human.push_str(&format!("    {m}\n"));
                }
                for c in cs["changes"].as_array().unwrap_or(&vec![]) {
                    human.push_str(&format!(
                        "    {} {}\n",
                        c["op"].as_str().unwrap_or(""),
                        c["path"].as_str().unwrap_or("")
                    ));
                }
            }
            Ok(Output::text(v, human.trim_end().to_string()))
        }
        Command::Show { changeset, r#ref } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            Ok(Output::json(query::show(
                &mut repo,
                &ref_name,
                parse_cs(changeset)?,
            )?))
        }
        Command::Why {
            target,
            depth,
            r#ref,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            Ok(Output::json(query::why(
                &mut repo, &ref_name, target, *depth,
            )?))
        }
        Command::Impact {
            target,
            depth,
            r#ref,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let v = wisp_engine::impact::impact(&mut repo, &ref_name, target, *depth)?;
            let human = impact_human(&v);
            Ok(Output::text(v, human))
        }
        Command::Cat {
            target,
            version,
            r#ref,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let v = version
                .as_deref()
                .map(|s| {
                    s.parse::<ChangeId>()
                        .map_err(|_| WispError::usage("bad version"))
                })
                .transpose()?;
            let out = wisp_engine::headless::cat(&mut repo, &ref_name, target, v)?;
            let human = out
                .get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
                .or_else(|| out["data"]["content"].as_str().map(|s| s.to_string()));
            Ok(match human {
                Some(h) => Output::text(out, h),
                None => Output::json(out),
            })
        }
        Command::Revert { changeset, r#ref } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let actor = actor_of(&cli, &repo);
            Ok(Output::json(query::revert(
                &mut repo,
                &ref_name,
                parse_cs(changeset)?,
                &actor,
            )?))
        }
        Command::Mv { from, to, r#ref } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let actor = actor_of(&cli, &repo);
            Ok(Output::json(query::mv(
                &mut repo, &ref_name, from, to, &actor,
            )?))
        }
        Command::Lifecycle {
            path,
            status,
            r#ref,
        } => set_status(&cli, path, status, r#ref),
        Command::Deprecate { path, r#ref } => set_status(&cli, path, "deprecated", r#ref),
        Command::Verify { path, r#ref } => {
            let mut repo = open(&cli)?;
            let op = empty_op("verify", path);
            let w = WriteFlags {
                r#ref: r#ref.clone(),
                ..Default::default()
            };
            let b = batch_for(&cli, &repo, &w, op)?;
            Ok(Output::json(wisp_engine::headless::apply(
                &mut repo, b, None,
            )?))
        }
        Command::Retract {
            path,
            reason,
            note,
            r#ref,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let actor = actor_of(&cli, &repo);
            Ok(Output::json(wisp_engine::erase::retract(
                &mut repo,
                &ref_name,
                path,
                parse_reason(reason)?,
                note.clone(),
                &actor,
            )?))
        }
        Command::Restore { path, r#ref } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let actor = actor_of(&cli, &repo);
            Ok(Output::json(wisp_engine::erase::restore(
                &mut repo, &ref_name, path, &actor,
            )?))
        }
        Command::Erase {
            target,
            group,
            actor_target,
            reason,
            yes,
            discard,
            r#ref,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let actor = actor_of(&cli, &repo);
            let t = match (target, group, actor_target) {
                (Some(t), None, None) => EraseTarget::Path(t.clone()),
                (None, Some(g), None) => EraseTarget::Group(g.clone()),
                (None, None, Some(a)) => EraseTarget::Actor(a.clone()),
                _ => {
                    return Err(WispError::usage(
                        "erase <path>[#section] | --group <name> | --actor-id <actor>",
                    ))
                }
            };
            let v = wisp_engine::erase::erase(
                &mut repo,
                &ref_name,
                &t,
                parse_reason(reason)?,
                *yes,
                *discard,
                &actor,
            )?;
            let pending = v["pending_subscribers"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            let human = if v["dry_run"] == true {
                format!(
                    "Would erase {} scope(s); pass --yes to proceed:\n{}",
                    v["scopes"],
                    serde_json::to_string_pretty(&v["would_erase"]).unwrap_or_default()
                )
            } else {
                format!(
                    "Erased {} scope(s) in changeset {}",
                    v["scopes"].as_array().map(|a| a.len()).unwrap_or(0),
                    v["changeset"]
                )
            };
            let out = Output::text(v, human);
            Ok(if pending {
                out.with_exit(ExitCode::ErasePending)
            } else {
                out
            })
        }
        Command::Group { cmd } => {
            let mut repo = open(&cli)?;
            match cmd {
                GroupCmd::Create { name } => Ok(Output::json(wisp_engine::erase::group_create(
                    &mut repo, name,
                )?)),
                GroupCmd::List => Ok(Output::json(wisp_engine::erase::group_list(&repo)?)),
                GroupCmd::Add {
                    name,
                    target,
                    r#ref,
                } => {
                    let ref_name = ref_of(&repo, r#ref);
                    let actor = actor_of(&cli, &repo);
                    Ok(Output::json(wisp_engine::erase::group_member(
                        &mut repo, &ref_name, name, target, true, &actor,
                    )?))
                }
                GroupCmd::Remove {
                    name,
                    target,
                    r#ref,
                } => {
                    let ref_name = ref_of(&repo, r#ref);
                    let actor = actor_of(&cli, &repo);
                    Ok(Output::json(wisp_engine::erase::group_member(
                        &mut repo, &ref_name, name, target, false, &actor,
                    )?))
                }
            }
        }
        Command::Source { cmd } => {
            let mut repo = open(&cli)?;
            match cmd {
                SourceCmd::Add {
                    path,
                    id,
                    resource,
                    title,
                    author,
                    r#ref,
                } => {
                    let ref_name = ref_of(&repo, r#ref);
                    let (c, _) = wisp_engine::erase::resolve_target(&mut repo, &ref_name, path)?;
                    let n = {
                        let hot = repo.hot(&ref_name)?;
                        wisp_store::Hot::sources_of(hot.conn(), c.concept)?.len()
                    };
                    let id = id.clone().unwrap_or_else(|| format!("src-{}", n + 1));
                    let mut rec = json!({ "id": id });
                    if let Some(r) = resource {
                        rec["resource"] = json!(r);
                    }
                    if let Some(t) = title {
                        rec["title"] = json!(t);
                    }
                    if let Some(a) = author {
                        rec["author"] = json!(a);
                    }
                    let mut op = empty_op("source_put", path);
                    op.key = Some(id);
                    op.value = Some(rec);
                    let w = WriteFlags {
                        r#ref: Some(ref_name),
                        ..Default::default()
                    };
                    let b = batch_for(&cli, &repo, &w, op)?;
                    Ok(Output::json(wisp_engine::headless::apply(
                        &mut repo, b, None,
                    )?))
                }
                SourceCmd::Rm { path, id, r#ref } => {
                    let mut op = empty_op("source_remove", path);
                    op.key = Some(id.clone());
                    let w = WriteFlags {
                        r#ref: r#ref.clone(),
                        ..Default::default()
                    };
                    let b = batch_for(&cli, &repo, &w, op)?;
                    Ok(Output::json(wisp_engine::headless::apply(
                        &mut repo, b, None,
                    )?))
                }
                SourceCmd::Ls { path, r#ref } => {
                    let ref_name = ref_of(&repo, r#ref);
                    let (c, _) = wisp_engine::erase::resolve_target(&mut repo, &ref_name, path)?;
                    let hot = repo.hot(&ref_name)?;
                    let v: Vec<Value> = wisp_store::Hot::sources_of(hot.conn(), c.concept)?.into_iter().map(|s| json!({ "id": s.source_id, "record": s.record, "version": s.version.to_string() })).collect();
                    Ok(Output::json(json!(v)))
                }
            }
        }
        Command::Branch { name, from } => {
            let mut repo = open(&cli)?;
            match name {
                Some(n) => Ok(Output::json(query::branch(&mut repo, n, from.as_deref())?)),
                None => Ok(Output::json(query::refs(&repo)?)),
            }
        }
        Command::Switch { name, discard } => {
            let mut repo = open(&cli)?;
            Ok(Output::json(query::switch(&mut repo, name, *discard)?))
        }
        Command::Merge {
            source,
            into,
            resolutions,
            abort,
        } => {
            let mut repo = open(&cli)?;
            let into_name = into.clone().unwrap_or_else(|| query::current_ref(&repo));
            Ok(Output::json(wisp_merge::merge(
                &mut repo,
                source,
                &into_name,
                resolutions.as_deref(),
                *abort,
            )?))
        }
        Command::Worktree { cmd } => {
            let mut repo = open(&cli)?;
            match cmd {
                WorktreeCmd::Add {
                    name,
                    branch,
                    from,
                    path,
                } => Ok(Output::json(wisp_engine::worktree::add(
                    &mut repo,
                    name,
                    branch.as_deref(),
                    from.as_deref(),
                    path.as_deref(),
                )?)),
                WorktreeCmd::List => Ok(Output::json(wisp_engine::worktree::list(&repo)?)),
                WorktreeCmd::Remove { name, force } => Ok(Output::json(
                    wisp_engine::worktree::remove(&mut repo, name, *force)?,
                )),
            }
        }
        Command::Remote { cmd } => {
            let mut repo = open(&cli)?;
            match cmd {
                RemoteCmd::Add { name, url, trusted } => {
                    repo.manifest.remotes.retain(|r| &r.name != name);
                    repo.manifest
                        .remotes
                        .push(wisp_engine::manifest::RemoteSection {
                            name: name.clone(),
                            url: url.clone(),
                            trusted: *trusted,
                        });
                    repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
                    Ok(Output::json(
                        json!({ "remote": name, "url": url, "trusted": trusted }),
                    ))
                }
                RemoteCmd::List => Ok(Output::json(serde_json::to_value(&repo.manifest.remotes)?)),
                RemoteCmd::Remove { name } => {
                    repo.manifest.remotes.retain(|r| &r.name != name);
                    repo.manifest.save(&repo.wisp_dir.join("manifest.toml"))?;
                    Ok(Output::json(json!({ "removed": name })))
                }
            }
        }
        Command::Push { remote, r#ref } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let url = remote_url(&repo, remote)?;
            let target = wisp_sync::parse_remote_url(&url)?;
            Ok(Output::json(wisp_sync::push_remote(
                &mut repo,
                &target,
                &ref_name,
                &unlock(&cli),
            )?))
        }
        Command::Pull { remote, r#ref } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let url = remote_url(&repo, remote)?;
            let target = wisp_sync::parse_remote_url(&url)?;
            Ok(Output::json(wisp_sync::pull_remote(
                &mut repo,
                &target,
                &ref_name,
                &unlock(&cli),
            )?))
        }
        Command::Serve { stdio, http } => {
            let unlock = unlock(&cli);
            if let Some(bind) = http {
                let repo = open(&cli)?;
                wisp_sync::serve_http(bind, &repo.root, &unlock)?;
                return Ok(Output::text(
                    json!({ "serve": "http", "bind": bind, "pack": "/wisp/v1/pack.zst" }),
                    format!("HTTP pack server stopped ({bind})"),
                ));
            }
            if !stdio {
                return Err(WispError::usage("serve requires --stdio or --http <addr>"));
            }
            let mut repo = open(&cli)?;
            Ok(Output::json(wisp_sync::serve_stdio(&mut repo, &unlock)?))
        }
        Command::Validate {
            path,
            strict,
            r#ref,
        } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            Ok(Output::json(query::validate(
                &mut repo,
                &ref_name,
                path.as_deref(),
                *strict,
            )?))
        }
        Command::Migrate { to, dry_run } => {
            let mut repo = open(&cli)?;
            let ref_name = query::current_ref(&repo);
            Ok(Output::json(query::migrate(
                &mut repo, &ref_name, to, *dry_run,
            )?))
        }
        Command::Export {
            git,
            archive,
            html,
            no_content,
            since,
            ids_only,
            sign,
            verify,
            profile,
            r#ref,
            strip_anchors,
        } => {
            if let Some(dir) = verify {
                return Ok(Output::json(wisp_engine::html_export::verify_html_export(
                    dir,
                )?));
            }
            let kinds = [git.is_some(), archive.is_some(), html.is_some()];
            if kinds.iter().filter(|&x| *x).count() != 1 {
                return Err(WispError::usage(
                    "export: specify exactly one of --git <dir>, --archive <file>, --html <dir>, or --verify <dir>",
                ));
            }
            if *no_content && html.is_none() {
                return Err(WispError::usage(
                    "--no-content applies only to --html exports",
                ));
            }
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            if let Some(dir) = git {
                return Ok(Output::json(wisp_git::export_git(
                    &mut repo, &ref_name, dir,
                )?));
            }
            if let Some(file) = archive {
                return Ok(Output::json(wisp_git::export_archive(
                    &mut repo,
                    &ref_name,
                    file,
                    profile.as_deref(),
                    *strip_anchors,
                )?));
            }
            let dir = html.as_ref().expect("checked above");
            let opts = wisp_html::HtmlExportOptions {
                content: !*no_content,
                since: since.clone(),
                ids_only: *ids_only,
                graph_depth: wisp_html::DEFAULT_GRAPH_DEPTH,
                graph_max_nodes: wisp_html::DEFAULT_GRAPH_MAX_NODES,
                sign: *sign,
            };
            let data = wisp_engine::html_export::export_html(&mut repo, &ref_name, dir, opts)?;
            let human = format!(
                "HTML export written to {}\nWarning: {}",
                data["dir"].as_str().unwrap_or(""),
                wisp_html::PLAINTEXT_NOTICE
            );
            Ok(Output::text(data, human))
        }
        Command::Index { rebuild } => {
            let mut repo = open(&cli)?;
            let built = query::index(&mut repo, rebuild.as_deref())?;
            let external = wisp_feed::run_external_subscribers(&mut repo)?;
            let mut out = built;
            if let Some(ext) = external.get("subscribers").and_then(|v| v.as_array()) {
                if let Some(subs) = out.get_mut("subscribers").and_then(|v| v.as_array_mut()) {
                    subs.extend(ext.clone());
                }
            }
            Ok(Output::json(out))
        }
        Command::Feed { since } => {
            let repo = open(&cli)?;
            let events = query::feed(&repo, *since)?;
            // Streaming command: one JSON object per line regardless of --json.
            for e in &events {
                println!("{}", serde_json::to_string(e).unwrap_or_default());
            }
            Ok(Output {
                data: json!({ "events": events.len() }),
                human: Some(String::new()),
                exit: ExitCode::Success,
            })
        }
        Command::Search {
            query: q,
            mode,
            k,
            threshold,
            local_only,
            codec,
            r#ref,
        } => {
            let _ = codec;
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let params = wisp_engine::search::SearchParams {
                threshold: *threshold,
                local_only: *local_only,
            };
            let v = query::search_with_params(&mut repo, &ref_name, q, mode, *k, &params)?;
            let human = v["results"]
                .as_array()
                .map(|rs| {
                    rs.iter()
                        .map(|r| {
                            format!(
                                "{:>8.3}  {}#{}  {}",
                                r["score"].as_f64().unwrap_or(0.0),
                                r["path"].as_str().unwrap_or(""),
                                r["section"].as_str().unwrap_or(""),
                                r["heading"].as_str().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            Ok(Output::text(v, human))
        }
        Command::Pack {
            paths,
            budget,
            codec,
            r#ref,
        } => {
            let _ = codec;
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            let v = query::pack(&mut repo, &ref_name, paths, *budget)?;
            Ok(Output {
                human: Some(serde_json::to_string(&v).unwrap_or_default()),
                data: v,
                exit: ExitCode::Success,
            })
        }
        Command::Tokens { target, r#ref } => {
            let mut repo = open(&cli)?;
            let ref_name = ref_of(&repo, r#ref);
            Ok(Output::json(query::tokens(&mut repo, &ref_name, target)?))
        }
        Command::Codec { action, id } => {
            let mut repo = open(&cli)?;
            Ok(Output::json(query::codec(
                &mut repo,
                action,
                id.as_deref(),
            )?))
        }
        Command::Fsck => {
            let mut repo = open(&cli)?;
            let v = query::fsck(&mut repo)?;
            Ok(Output::text(
                v.clone(),
                format!(
                    "ok: {} changesets, {} payloads verified",
                    v["changesets"], v["payloads"]
                ),
            ))
        }
        Command::Gc => {
            let mut repo = open(&cli)?;
            Ok(Output::json(query::gc(&mut repo)?))
        }
        Command::Config { action, key, value } => {
            let mut repo = open(&cli)?;
            match action.as_str() {
                "get" => Ok(Output::json(query::config(&mut repo, key, None)?)),
                "set" => Ok(Output::json(query::config(
                    &mut repo,
                    key,
                    Some(
                        value
                            .as_deref()
                            .ok_or_else(|| WispError::usage("config set <key> <value>"))?,
                    ),
                )?)),
                _ => Err(WispError::usage("config get|set <key> [value]")),
            }
        }
        Command::Actor { cmd } => {
            let mut repo = open(&cli)?;
            match cmd {
                ActorCmd::List => Ok(Output::json(query::actors(&repo)?)),
                ActorCmd::GenKey { actor, r#ref } => {
                    let ref_name = ref_of(&repo, r#ref);
                    Ok(Output::json(query::actor_gen_key(
                        &mut repo, &ref_name, actor,
                    )?))
                }
                ActorCmd::Key {
                    actor,
                    public_key_file,
                    r#ref,
                } => {
                    let ref_name = ref_of(&repo, r#ref);
                    let bytes = std::fs::read(public_key_file)?;
                    Ok(Output::json(query::actor_key(
                        &mut repo, &ref_name, actor, &bytes,
                    )?))
                }
            }
        }
        Command::Hot { cmd } => {
            let mut repo = open(&cli)?;
            match cmd {
                HotCmd::Rebuild { r#ref } => {
                    let ref_name = ref_of(&repo, r#ref);
                    Ok(Output::json(query::hot_rebuild(&mut repo, &ref_name)?))
                }
                HotCmd::Check { r#ref } => {
                    let ref_name = ref_of(&repo, r#ref);
                    Ok(Output::json(query::hot_check(&mut repo, &ref_name)?))
                }
            }
        }
        Command::Backup { dest, with_keys } => {
            let repo = open(&cli)?;
            Ok(Output::json(query::backup(
                &repo,
                dest,
                with_keys.as_deref(),
            )?))
        }
        Command::RestoreBackup { src, keys, into } => {
            let into = into.clone().unwrap_or(std::env::current_dir()?);
            let v = query::restore_backup(src, keys.as_deref(), &into)?;
            // Rebuild the hot tier and working tree.
            let mut repo = Repository::open(&into, &unlock(&cli))?;
            wisp_engine::checkout::checkout(&mut repo, wisp_engine::MAIN_REF, None, false, false)?;
            Ok(Output::json(v))
        }
        Command::Update {
            target,
            w,
            heading,
            level,
            content,
            content_file,
            stdin,
        } => {
            let mut repo = open(&cli)?;
            let (path, section) = split_target(target);
            let mut op = empty_op("update_section", &path);
            op.section = section;
            op.heading = heading.clone();
            op.level = *level;
            op.content = read_content(content.clone(), content_file.clone(), *stdin)?;
            let b = batch_for(&cli, &repo, w, op)?;
            Ok(Output::json(wisp_engine::headless::apply(
                &mut repo, b, None,
            )?))
        }
        Command::AddSection {
            path,
            w,
            after,
            heading,
            level,
            content,
            content_file,
            stdin,
        } => {
            let mut repo = open(&cli)?;
            let mut op = empty_op("add_section", path);
            op.after = after.clone();
            op.heading = Some(heading.clone());
            op.level = Some(*level);
            op.content = read_content(content.clone(), content_file.clone(), *stdin)?;
            let b = batch_for(&cli, &repo, w, op)?;
            Ok(Output::json(wisp_engine::headless::apply(
                &mut repo, b, None,
            )?))
        }
        Command::RemoveSection { target, w } => {
            let mut repo = open(&cli)?;
            let (path, section) = split_target(target);
            let mut op = empty_op("remove_section", &path);
            op.section = section;
            let b = batch_for(&cli, &repo, w, op)?;
            Ok(Output::json(wisp_engine::headless::apply(
                &mut repo, b, None,
            )?))
        }
        Command::Set {
            path,
            key,
            value,
            w,
        } => {
            let mut repo = open(&cli)?;
            let v: Value =
                serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone()));
            let mut op = empty_op("set_key", path);
            op.key = Some(key.clone());
            op.value = Some(v);
            let b = batch_for(&cli, &repo, w, op)?;
            Ok(Output::json(wisp_engine::headless::apply(
                &mut repo, b, None,
            )?))
        }
        Command::Unset { path, key, w } => {
            let mut repo = open(&cli)?;
            let mut op = empty_op("unset_key", path);
            op.key = Some(key.clone());
            let b = batch_for(&cli, &repo, w, op)?;
            Ok(Output::json(wisp_engine::headless::apply(
                &mut repo, b, None,
            )?))
        }
        Command::Create {
            path,
            r#type,
            frontmatter_file,
            content_file,
            content,
            w,
        } => {
            let mut repo = open(&cli)?;
            let mut op = empty_op("create_concept", path);
            op.r#type = Some(r#type.clone());
            if let Some(f) = frontmatter_file {
                let text = std::fs::read_to_string(f)?;
                let yv: serde_yaml_value::Value = serde_yaml_value::from_str(&text)?;
                op.frontmatter = Some(serde_json::to_value(yv)?);
            }
            op.content = read_content(content.clone(), content_file.clone(), false)?;
            let b = batch_for(&cli, &repo, w, op)?;
            Ok(Output::json(wisp_engine::headless::apply(
                &mut repo, b, None,
            )?))
        }
        Command::Apply {
            file,
            r#ref,
            resolve,
        } => {
            let mut repo = open(&cli)?;
            let text = match file {
                Some(f) => std::fs::read_to_string(f)?,
                None => {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                    s
                }
            };
            let mut batch: ApplyBatch =
                serde_json::from_str(&text).map_err(|e| WispError::usage(format!("apply: {e}")))?;
            if batch.actor.is_none() {
                batch.actor = Some(actor_of(&cli, &repo));
            }
            let default_ref = r#ref.clone().unwrap_or_else(|| query::current_ref(&repo));
            if *resolve {
                let policy = wisp_engine::headless::ApplyResolvePolicy::from_manifest(
                    repo.manifest.commit.retries,
                );
                let (result, stats) = wisp_engine::headless::apply_with_resolve(
                    &mut repo,
                    batch,
                    Some(&default_ref),
                    &policy,
                )?;
                let mut out = result;
                if let Some(obj) = out.as_object_mut() {
                    obj.insert(
                        "resolve".into(),
                        serde_json::json!({
                            "attempts": stats.attempts,
                            "conflict_rounds": stats.conflict_rounds,
                            "retryable_errors": stats.retryable_errors,
                        }),
                    );
                }
                Ok(Output::json(out))
            } else {
                Ok(Output::json(wisp_engine::headless::apply(
                    &mut repo,
                    batch,
                    Some(&default_ref),
                )?))
            }
        }
    }
}

fn set_status(cli: &Cli, path: &str, status: &str, r#ref: &Option<String>) -> CliResult {
    let mut repo = open(cli)?;
    let mut op = empty_op("set_status", path);
    op.value = Some(json!(status));
    let w = WriteFlags {
        r#ref: r#ref.clone(),
        ..Default::default()
    };
    let b = batch_for(cli, &repo, &w, op)?;
    Ok(Output::json(wisp_engine::headless::apply(
        &mut repo, b, None,
    )?))
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Minimal YAML front door for `--frontmatter-file` without adding a dependency to the CLI crate.
mod serde_yaml_value {
    pub use serde_json::Value;
    pub fn from_str(s: &str) -> Result<Value, wisp_model::WispError> {
        // JSON is valid YAML; for YAML files, go through the engine's parser.
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            return Ok(v);
        }
        wisp_engine::yaml_to_json(s)
    }
}
