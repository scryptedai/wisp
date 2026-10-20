//! Realistic desk playbook: SPY calls from news signals, honest mistakes, multi-agent writes.
//!
//! Fixture: `wisp-corpus/data/bundles/spy-options/`
//! Narrative: `wisp-corpus/data/scenarios/playbook/spy-options/README.md`

mod spy_harness;

use spy_harness::*;
use wisp_engine::headless::ApplyInput;
use wisp_engine::record::{record, RecordOptions};
use wisp_engine::repo::MAIN_REF;
use wisp_invariants::catalog;
use wisp_model::{ChangeId, ExitCode, Reason};
use wisp_parse::parse_file;
use wisp_store::Hot;

#[test]
fn spy_options_news_signal_multi_agent_playbook() {
    let cat = catalog();
    let spec = cat.bundle("spy-options").expect("spy-options bundle");
    let (_dir, root, mut repo) = fresh_spy_repo(spec);

    let strategy = "strategy/spy-calls-overview";
    let (entry_sec, entry_ver) = section_at(&mut repo, strategy, "Entry rules");
    let (exit_sec, _exit_ver) = section_at(&mut repo, strategy, "Exit rules");
    let (thesis_sec, thesis_ver) = section_at(&mut repo, strategy, "Thesis");

    // Agent A: fold CPI print into entry rules (provenance from observation).
    let cpi_input = ApplyInput {
        concept: "observations/2026-10-18-cpi-print".into(),
        section: None,
        at: None,
    };
    let entry_after_cpi = r"- **VIX** below 22 at entry (no chase in panic vol).
- **News tier**: Tier-1 macro; **2026-10-18 CPI** dovish print — rubric +5, calls enabled.
- **Structure**: Prefer calls with delta 0.35–0.45; max premium at risk per position limits.
- **No entry** inside 15 minutes of a known Tier-1 print unless the plan was written the prior session.
";
    update_section(
        &mut repo,
        "process:news-scanner/1.0",
        strategy,
        entry_sec,
        entry_ver,
        entry_after_cpi,
        "CPI impulse: tighten entry checklist",
        vec![cpi_input.clone()],
    )
    .unwrap();

    // Agent B: disjoint section — desk risk caps (no conflict with A).
    let (limits_sec, limits_ver) = section_at(&mut repo, "risk/position-limits", "Per trader");
    let limits_body = r"- Max **$20k** premium at risk in SPY calls per session (temporary cut post-CPI vol).
- Max **3** concurrent structures.
";
    update_section(
        &mut repo,
        "process:risk-bot/0.2",
        "risk/position-limits",
        limits_sec,
        limits_ver,
        limits_body,
        "Post-CPI: tighten per-trader cap",
        vec![],
    )
    .unwrap();

    // Agents C & D: both touch Exit rules — classic honest mistake.
    let (_, exit_v1) = section_at(&mut repo, strategy, "Exit rules");
    let exec_exit = r"- Take **50%** off at +40% premium; trail remainder **-25%** from peak.
- Flatten before next Tier-1 unless carried with written rationale.
- **Desk rule**: no new adds after +60% extension without human verify.
";
    update_section(
        &mut repo,
        "process:execution-bot/1.0",
        strategy,
        exit_sec,
        exit_v1,
        exec_exit,
        "Execution: add extension guardrail",
        vec![],
    )
    .unwrap();

    let scanner_exit = r"- Take **50%** off at +35% premium (faster scale-out on news trades).
- Flatten all SPY call exposure **15 minutes before** the next Tier-1 event.
- If thesis invalidated, exit at market.
";
    let err = update_section(
        &mut repo,
        "process:news-scanner/1.0",
        strategy,
        exit_sec,
        exit_v1, // stale — execution-bot already moved the section
        scanner_exit,
        "News: prefer faster scale-out",
        vec![cpi_input.clone()],
    )
    .unwrap_err();
    assert_eq!(err.code, ExitCode::Conflict);
    let conflicts = &err.details["conflicts"];
    assert_eq!(conflicts[0]["type"], "section_conflict");
    assert_eq!(conflicts[0]["path"], "strategy/spy-calls-overview.md");
    let theirs: ChangeId = conflicts[0]["theirs"]["version"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // Scanner synthesizes: blend both intents in agent context, resubmit on theirs.
    let merged_exit = r"- Take **50%** off at +35% premium; trail remainder **-25%** from peak.
- Flatten before next Tier-1 unless carried with written rationale.
- **Desk rule**: no new adds after +60% extension without human verify.
- If thesis invalidated, exit at market.
";
    update_section(
        &mut repo,
        "process:news-scanner/1.0",
        strategy,
        exit_sec,
        theirs,
        merged_exit,
        "Resolved exit conflict after CPI session",
        vec![cpi_input.clone()],
    )
    .unwrap();

    // Human honest mistake: reorder sections in the working tree (Entry before Thesis).
    let overview_path = root.join("strategy/spy-calls-overview.md");
    let text = std::fs::read_to_string(&overview_path).unwrap();
    let parsed = parse_file(&text);
    let thesis_heading = parsed
        .sections()
        .iter()
        .find(|s| s.heading == "Thesis")
        .expect("thesis");
    let entry_heading = parsed
        .sections()
        .iter()
        .find(|s| s.heading == "Entry rules")
        .expect("entry");
    let thesis_id = match &thesis_heading.anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!("thesis anchor"),
    };
    let entry_id = match &entry_heading.anchor {
        Some(wisp_parse::Anchor::Valid(id)) => *id,
        _ => panic!("entry anchor"),
    };
    let mut lines = text.lines().collect::<Vec<_>>();
    let thesis_start = lines.iter().position(|l| l.contains("## Thesis")).unwrap();
    let entry_start = lines.iter().position(|l| l.contains("## Entry rules")).unwrap();
    assert!(thesis_start < entry_start);
    let entry_block = lines.split_off(entry_start);
    let thesis_start = lines.iter().position(|l| l.contains("## Thesis")).unwrap();
    let thesis_block = lines.split_off(thesis_start);
    let mut new_lines = lines;
    new_lines.extend(entry_block);
    new_lines.extend(thesis_block);
    std::fs::write(&overview_path, new_lines.join("\n")).unwrap();
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:tim".into()),
            message: Some("Reorder: entry rules before thesis for morning standup".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let hot = repo.hot(MAIN_REF).unwrap();
    let thesis_after = Hot::section(hot.conn(), thesis_id).unwrap().unwrap();
    let entry_after = Hot::section(hot.conn(), entry_id).unwrap().unwrap();
    assert_eq!(thesis_after.section, thesis_id);
    assert_eq!(entry_after.section, entry_id);
    assert!(entry_after.idx < thesis_after.idx, "Entry should sort before Thesis");

    // Retract stale pre-CPI whisper observation (superseded by CPI print).
    wisp_engine::erase::retract(
        &mut repo,
        MAIN_REF,
        "observations/2026-10-17-premarket",
        Reason::Superseded,
        Some("CPI print on 2026-10-18 superseded PPI whisper".into()),
        "human:tim",
    )
    .unwrap();

    // Human edits thesis in the file without recording; agent updates journal (I19).
    let before_thesis = std::fs::read_to_string(&overview_path).unwrap();
    std::fs::write(
        &overview_path,
        before_thesis.replace(
            "We buy **0–2 DTE",
            "We buy **0–2 DTE** (desk emphasis:",
        ),
    )
    .unwrap();
    let (journal_sec, journal_ver) = section_at(&mut repo, "journal/trade-log", "Open positions");
    update_section(
        &mut repo,
        "process:execution-bot/1.0",
        "journal/trade-log",
        journal_sec,
        journal_ver,
        "- **SPY 10/20 585C** — CPI entry; stop −25% from peak; sized per tightened risk cap.\n",
        "Log: reflect risk cap after CPI",
        vec![cpi_input.clone()],
    )
    .unwrap();
    let dirty = std::fs::read_to_string(&overview_path).unwrap();
    assert!(dirty.contains("desk emphasis"), "local thesis edit preserved");
    assert!(!dirty.contains("sized per tightened"), "journal not in strategy file");
    record(
        &mut repo,
        &RecordOptions {
            actor: Some("human:tim".into()),
            message: Some("Commit thesis emphasis after agent journal update".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let final_thesis = std::fs::read_to_string(&overview_path).unwrap();
    assert!(final_thesis.contains("desk emphasis"));
    assert!(final_thesis.contains("sized per tightened") == false);

    assert!(wisp_engine::query::fsck(&mut repo).is_ok());
    assert!(wisp_engine::query::hot_check(&mut repo, MAIN_REF).is_ok());

    let _ = (thesis_sec, thesis_ver); // recorded path used anchors from first record
}
