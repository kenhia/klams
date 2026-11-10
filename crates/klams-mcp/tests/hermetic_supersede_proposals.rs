//! Sprint 058 (klams-mind WI 3331): the propose-supersede path.
//!
//! `supersede_propose` files, `supersede_proposals_list` reads the
//! queue, `supersede_proposal_resolve` accepts or rejects. These pin the
//! handler behaviour against `MemStore`; the Postgres half (dedupe index,
//! conditional transition) is exercised in `klams-service`'s docker suite.

mod support;

use klams_mcp::tools::{
    memory_get::{run as memory_get, MemoryGetArgs},
    supersede_proposal_resolve::{
        run as resolve, ResolveDecision, ResolveOutcome, SupersedeProposalResolveArgs,
    },
    supersede_proposals_list::{run as list, ProposalStatusFilter, SupersedeProposalsListArgs},
    supersede_propose::{run as propose, SupersedeProposeArgs},
};
use klams_types::SupersedeProposalStatus;
use support::{curator, state, writer};
use uuid::Uuid;

fn args(keeper: Uuid, retire: Vec<Uuid>, text: Option<&str>) -> SupersedeProposeArgs {
    SupersedeProposeArgs {
        keeper_id: keeper,
        retire_ids: retire,
        text: text.map(str::to_string),
        reason: "duplicates of one another".into(),
    }
}

fn accept(ids: Vec<Uuid>) -> SupersedeProposalResolveArgs {
    SupersedeProposalResolveArgs {
        proposal_ids: ids,
        decision: ResolveDecision::Accept,
        note: None,
    }
}

// ------------------------------------------------------------- propose

#[tokio::test]
async fn propose_files_a_pending_proposal_and_changes_no_memory() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let keeper = store.seed_knowledge(claude, "kubs0 runs klams");
    let dup = store.seed_knowledge(claude, "klams runs on kubs0");

    let out = propose(&st, args(keeper, vec![dup], None), Some(&writer(mind)))
        .await
        .expect("a writer may propose against another author's records");
    assert!(!out.deduped);
    assert_eq!(out.proposal.status, SupersedeProposalStatus::Pending);
    assert_eq!(out.proposal.author_id, mind);
    assert!(!store.is_knowledge_soft_deleted(dup), "nothing changes yet");
}

#[tokio::test]
async fn propose_dedupes_an_identical_pending_proposal() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let keeper = store.seed_knowledge(claude, "a");
    let dup = store.seed_knowledge(claude, "b");
    let first = propose(&st, args(keeper, vec![dup], None), Some(&writer(mind)))
        .await
        .unwrap();
    let again = propose(&st, args(keeper, vec![dup], None), Some(&writer(mind)))
        .await
        .unwrap();
    assert!(again.deduped);
    assert_eq!(again.proposal.id, first.proposal.id);
    assert_eq!(again.proposal.submission_count, 2);
}

#[tokio::test]
async fn propose_refuses_shapes_that_cannot_be_applied() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let keeper = store.seed_knowledge(claude, "a");
    let dup = store.seed_knowledge(claude, "b");
    let w = writer(mind);

    for (bad, why) in [
        (args(keeper, vec![], None), "empty retire set"),
        (args(keeper, vec![keeper], None), "keeper in retire set"),
        (args(keeper, vec![dup, dup], None), "duplicate retiree"),
        (args(keeper, vec![dup], Some("   ")), "blank text"),
        (
            SupersedeProposeArgs {
                reason: " ".into(),
                ..args(keeper, vec![dup], None)
            },
            "blank reason",
        ),
        (
            args(keeper, (0..21).map(|_| Uuid::now_v7()).collect(), None),
            "too many",
        ),
    ] {
        let err = propose(&st, bad, Some(&w)).await.expect_err(why);
        assert_eq!(err.meta.error_code, "SCHEMA_VALIDATION_FAILED", "{why}");
    }

    let err = propose(&st, args(keeper, vec![Uuid::now_v7()], None), Some(&w))
        .await
        .expect_err("unknown retiree");
    assert_eq!(err.meta.error_code, "NOT_FOUND");

    let err = propose(&st, args(keeper, vec![dup], None), None)
        .await
        .expect_err("no bound author");
    assert_eq!(err.meta.error_code, "MISSING_AUTHOR_ID");
}

#[tokio::test]
async fn propose_refuses_an_already_deleted_record() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let keeper = store.seed_knowledge(claude, "a");
    let dup = store.seed_knowledge(claude, "b");
    store.mark_deleted(dup, claude);
    let err = propose(&st, args(keeper, vec![dup], None), Some(&writer(mind)))
        .await
        .expect_err("a deleted record cannot be retired again");
    assert_eq!(err.meta.error_code, "NOT_FOUND");
}

// -------------------------------------------------------------- accept

#[tokio::test]
async fn accept_without_text_retires_every_record_into_the_keeper() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let ghcp = store.seed_author("ghcp");
    let mind = store.seed_author("klams-mind");
    let ken = store.seed_author("ken");
    let keeper = store.seed_knowledge(claude, "keeper");
    let a = store.seed_knowledge(claude, "dup a");
    let b = store.seed_knowledge(ghcp, "dup b");
    let p = propose(&st, args(keeper, vec![a, b], None), Some(&writer(mind)))
        .await
        .unwrap()
        .proposal;

    let out = resolve(&st, accept(vec![p.id]), Some(&curator(ken)))
        .await
        .unwrap();
    assert_eq!(out.results[0].outcome, ResolveOutcome::Accepted);
    assert_eq!(out.results[0].result_memory_id, Some(keeper));

    for r in [a, b] {
        assert!(store.is_knowledge_soft_deleted(r));
        assert_eq!(store.superseded_by(r), Some(keeper), "lineage");
        assert_eq!(
            store.knowledge_deleted_by(r),
            Some(ken),
            "the resolver acted"
        );
        let err = memory_get(&st, MemoryGetArgs { id: r }).await.unwrap_err();
        assert_eq!(err.meta.error_code, "NOT_FOUND");
    }
    assert!(!store.is_knowledge_soft_deleted(keeper));

    let accepted = list(
        &st,
        SupersedeProposalsListArgs {
            status: Some(ProposalStatusFilter::Accepted),
            limit: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(accepted.proposals.len(), 1);
    assert_eq!(accepted.proposals[0].result_memory_id, Some(keeper));
    assert_eq!(accepted.proposals[0].resolved_by_author_id, Some(ken));
}

#[tokio::test]
async fn accept_with_text_writes_the_merge_as_the_proposer() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let ken = store.seed_author("ken");
    let keeper = store.seed_knowledge(claude, "half one");
    let other = store.seed_knowledge(claude, "half two");
    let p = propose(
        &st,
        args(keeper, vec![other], Some("both halves")),
        Some(&writer(mind)),
    )
    .await
    .unwrap()
    .proposal;

    let out = resolve(&st, accept(vec![p.id]), Some(&curator(ken)))
        .await
        .unwrap();
    let merged = out.results[0].result_memory_id.expect("a new memory");
    assert_ne!(merged, keeper);
    assert_eq!(store.knowledge_text(merged).as_deref(), Some("both halves"));
    assert_eq!(
        store.knowledge_author(merged),
        Some(mind),
        "proposer's words"
    );
    for r in [keeper, other] {
        assert_eq!(store.superseded_by(r), Some(merged));
    }
    let live = memory_get(&st, MemoryGetArgs { id: merged }).await.unwrap();
    assert_eq!(live.id, merged);
}

#[tokio::test]
async fn overlapping_proposals_go_stale_once_a_sibling_is_accepted() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let ken = store.seed_author("ken");
    let keeper = store.seed_knowledge(claude, "keeper");
    let a = store.seed_knowledge(claude, "a");
    let b = store.seed_knowledge(claude, "b");
    let w = writer(mind);
    // Two merges sharing a keeper, as in klams-mind's 2026-09-25 run.
    let p1 = propose(&st, args(keeper, vec![a], Some("keeper+a")), Some(&w))
        .await
        .unwrap()
        .proposal;
    let p2 = propose(&st, args(keeper, vec![b], Some("keeper+b")), Some(&w))
        .await
        .unwrap()
        .proposal;

    let out = resolve(&st, accept(vec![p1.id, p2.id]), Some(&curator(ken)))
        .await
        .unwrap();
    assert_eq!(out.results[0].outcome, ResolveOutcome::Accepted);
    assert_eq!(out.results[1].outcome, ResolveOutcome::Stale);
    assert!(!store.is_knowledge_soft_deleted(b), "stale changes nothing");

    let stale = list(
        &st,
        SupersedeProposalsListArgs {
            status: Some(ProposalStatusFilter::Stale),
            limit: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(stale.proposals.len(), 1);
    assert!(stale.proposals[0]
        .resolution_note
        .as_deref()
        .is_some_and(|n| n.contains(&keeper.to_string())));
}

#[tokio::test]
async fn a_failed_apply_rolls_back_and_returns_to_pending() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let ken = store.seed_author("ken");
    let keeper = store.seed_knowledge(claude, "keeper");
    let a = store.seed_knowledge(claude, "a");
    let b = store.seed_knowledge(claude, "b");
    let p = propose(&st, args(keeper, vec![a, b], None), Some(&writer(mind)))
        .await
        .unwrap()
        .proposal;
    // Retire ids are stored sorted; fail on the second so the first
    // must be rolled back.
    let second = *p.retire_ids.last().unwrap();
    let first = p.retire_ids[0];
    store.fail_mark_superseded_for(second);

    let out = resolve(&st, accept(vec![p.id]), Some(&curator(ken)))
        .await
        .unwrap();
    assert_eq!(out.results[0].outcome, ResolveOutcome::Failed);
    assert!(!store.is_knowledge_soft_deleted(first), "rolled back");
    let pending = list(&st, SupersedeProposalsListArgs::default())
        .await
        .unwrap();
    assert_eq!(pending.proposals.len(), 1, "back in the queue to retry");
}

// -------------------------------------------------------------- reject

#[tokio::test]
async fn reject_closes_the_proposal_and_a_second_resolve_is_not_pending() {
    let (st, store) = state();
    let claude = store.seed_author("claude");
    let mind = store.seed_author("klams-mind");
    let ken = store.seed_author("ken");
    let keeper = store.seed_knowledge(claude, "keeper");
    let a = store.seed_knowledge(claude, "a");
    let p = propose(&st, args(keeper, vec![a], None), Some(&writer(mind)))
        .await
        .unwrap()
        .proposal;

    let out = resolve(
        &st,
        SupersedeProposalResolveArgs {
            proposal_ids: vec![p.id, Uuid::now_v7()],
            decision: ResolveDecision::Reject,
            note: Some("not duplicates".into()),
        },
        Some(&curator(ken)),
    )
    .await
    .unwrap();
    assert_eq!(out.results[0].outcome, ResolveOutcome::Rejected);
    assert_eq!(out.results[1].outcome, ResolveOutcome::NotFound);
    assert!(!store.is_knowledge_soft_deleted(a));

    let again = resolve(&st, accept(vec![p.id]), Some(&curator(ken)))
        .await
        .unwrap();
    assert_eq!(again.results[0].outcome, ResolveOutcome::NotPending);
}

#[tokio::test]
async fn resolve_refuses_an_empty_batch() {
    let (st, store) = state();
    let ken = store.seed_author("ken");
    let err = resolve(&st, accept(vec![]), Some(&curator(ken)))
        .await
        .unwrap_err();
    assert_eq!(err.meta.error_code, "SCHEMA_VALIDATION_FAILED");
}

// --------------------------------------------------------------- scope

#[test]
fn resolve_needs_manage_and_propose_needs_only_write() {
    use klams_mcp::tools::required_scope;
    use klams_types::Scope;
    assert_eq!(
        required_scope("supersede_proposal_resolve"),
        Some(Scope::Manage)
    );
    assert_eq!(required_scope("supersede_propose"), Some(Scope::Write));
    assert_eq!(
        required_scope("supersede_proposals_list"),
        Some(Scope::Read)
    );
}
