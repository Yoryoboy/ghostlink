use destiny_core::mod_assignment::resolve_mod_cost_variant;

#[test]
fn dim_cost_variant_resolution_table() {
    // DIM mod-utils.ts:120-132: reduced wins, then original if unlocked,
    // then normal. An already-unlocked reduced original remains reduced.
    for (requested, unlocked, expected) in [
        (14520248, vec![], 14520248),
        (2318667184, vec![], 14520248),
        (14520248, vec![2318667184], 2318667184),
        (2318667184, vec![2318667184], 2318667184),
        (14520248, vec![14520248, 2318667184], 2318667184),
        (2318667184, vec![14520248], 14520248),
        (42, vec![], 42),
        (42, vec![42], 42),
    ] {
        assert_eq!(resolve_mod_cost_variant(requested, &unlocked), expected);
    }
}

#[test]
fn all_legacy_stat_mods_migrate_before_cost_resolution() {
    // DIM loadout-utils.ts:880-893, commit 29c122e.
    for (old, normal) in [
        (204137529, 1703647492),
        (3961599962, 4183296050),
        (3682186345, 2532323436),
        (2850583378, 1180408010),
        (555005975, 1237786518),
        (2645858828, 4204488676),
        (2623485440, 4021790309),
        (4048838440, 1435557120),
        (1227870362, 350061697),
        (3355995799, 2724608735),
        (3699676109, 2639422088),
        (3253038666, 4287799666),
    ] {
        assert_eq!(resolve_mod_cost_variant(old, &[]), normal);
    }
    assert_eq!(
        resolve_mod_cost_variant(3961599962, &[3808902618]),
        3808902618
    );
}
