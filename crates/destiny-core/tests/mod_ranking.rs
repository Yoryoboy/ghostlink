use destiny_core::mod_assignment::*;

fn item(capacity: i32, original: i32) -> RankingItem {
    RankingItem {
        original_capacity: original,
        derived_capacity: capacity,
        used: 0,
        specialty: None,
        current_mods: vec![],
        costs: vec![[0; 5]; 12],
        enhancement_cost: None,
    }
}

#[test]
fn dim_rank_priority_table() {
    for (name, cost, current, expected) in [
        ("Heap order breaks exact ties", [0; 5], false, 0),
        ("changes break cost ties", [0; 5], true, 1),
        ("cost before changes", [0, 0, 0, 0, 1], false, 1),
        ("fit before cost", [1, 0, 0, 0, 0], false, 0),
    ] {
        let mut items = vec![item(10, 1); 5];
        items[0].costs[5] = cost;
        if current {
            items[1].current_mods.push(42);
        }
        if name == "fit before cost" {
            for i in &mut items[1..] {
                i.derived_capacity = 1;
            }
        }
        let fit =
            rank_independent_mods(&items, &[AssignmentMod { hash: 42, cost: 5 }], &[]).unwrap();
        assert_eq!(fit.assigned[expected], vec![42], "{name}");
        assert!(fit.unassigned.is_empty(), "{name}");
    }
}

#[test]
fn dim_activity_before_general_and_zero_original_energy() {
    let mut items = vec![item(5, 1); 5];
    items[0].specialty = Some(13646368);
    for i in &mut items[1..] {
        i.original_capacity = 0;
    }
    let result = rank_independent_mods(
        &items,
        &[AssignmentMod { hash: 1, cost: 3 }],
        &[(AssignmentMod { hash: 2, cost: 4 }, 13646368)],
    )
    .unwrap();
    assert_eq!(result.assigned[0], vec![2]);
    assert_eq!(result.unassigned, vec![1]);
}

#[test]
fn dim_activity_specialty_and_bucket_energy_table() {
    for (specialty, used, expected_activity) in [
        (Some(13646368), 0, 0),
        (Some(13646368), 3, 0),
        (Some(123), 0, 1),
        (None, 0, 1),
    ] {
        let mut items = vec![item(7, 1); 5];
        items[0].specialty = specialty;
        items[0].used = used;
        items[1].specialty = Some(13646368);
        items[1].current_mods = vec![1, 2];
        // Keep both candidates on item 0 when possible by making item 1 more
        // costly. Bucket energy can force general onto a different item.
        for tier in &mut items[1].costs[4..] {
            *tier = [0, 0, 0, 0, 1];
        }
        let result = rank_independent_mods(
            &items,
            &[AssignmentMod { hash: 1, cost: 3 }],
            &[(AssignmentMod { hash: 2, cost: 4 }, 13646368)],
        )
        .unwrap();
        assert!(result.unassigned.is_empty());
        assert!(result.assigned[expected_activity].contains(&2));
        assert_eq!(
            result
                .assigned
                .iter()
                .flatten()
                .filter(|h| **h == 1)
                .count(),
            1
        );
        if used == 3 {
            assert!(!result.assigned[0].contains(&1));
        }
    }
}

#[test]
fn dim_lexicographic_cost_and_paid_tiers() {
    let mut items = vec![item(10, 1); 5];
    items[0].costs[5] = [0, 1, 0, 0, 0];
    items[1].costs[5] = [0, 0, 99, 0, 0];
    items[2].costs[1] = [0, 0, 100, 0, 0];
    items[2].costs[5] = [0, 0, 100, 0, 1];
    items[3].derived_capacity = 1;
    items[4].derived_capacity = 1;
    let result =
        rank_independent_mods(&items, &[AssignmentMod { hash: 42, cost: 5 }], &[]).unwrap();
    assert_eq!(result.assigned[2], vec![42]);
    assert_eq!(result.upgrade_costs, [0, 0, 0, 0, 1]);
}
