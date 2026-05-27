//! FR-019: same seed → byte-identical corpus.

use klams_bench::{
    canonical_facts_digest, canonical_knowledge_digest, generate_facts, generate_knowledge,
    DEFAULT_SEED,
};

#[test]
fn same_seed_produces_same_facts() {
    let a = generate_facts(DEFAULT_SEED, 256);
    let b = generate_facts(DEFAULT_SEED, 256);
    assert_eq!(canonical_facts_digest(&a), canonical_facts_digest(&b));
}

#[test]
fn same_seed_produces_same_knowledge() {
    let a = generate_knowledge(DEFAULT_SEED, 256);
    let b = generate_knowledge(DEFAULT_SEED, 256);
    assert_eq!(
        canonical_knowledge_digest(&a),
        canonical_knowledge_digest(&b)
    );
}

#[test]
fn different_seed_produces_different_facts() {
    let a = generate_facts(DEFAULT_SEED, 64);
    let b = generate_facts(DEFAULT_SEED ^ 0x01, 64);
    assert_ne!(canonical_facts_digest(&a), canonical_facts_digest(&b));
}
