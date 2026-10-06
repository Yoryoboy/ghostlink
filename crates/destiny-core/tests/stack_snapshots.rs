use destiny_core::*;

fn stack(container: StackContainer, hash: i64, bucket: i64, quantity: i64) -> StackObservation {
    StackObservation::new(
        ItemHash::new(hash),
        BucketHash::new(bucket),
        quantity,
        container,
    )
    .unwrap()
}

fn input() -> InventorySnapshotInput {
    InventorySnapshotInput {
        stacks: vec![
            stack(StackContainer::ProfileInventory, 7, 8, 2),
            stack(StackContainer::ProfileCurrencies, 7, 8, 3),
            stack(StackContainer::ProfileInventory, 7, 8, 2),
        ],
        plug_set_entries: Vec::new(),
        membership_id: MembershipId::new("123"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("v1"),
        checked_at: Timestamp::new(200),
        source: SourceMarker::Profile,
        characters: Vec::new(),
        items: Vec::new(),
    }
}

#[test]
fn stack_snapshot_metadata_conversion_preserves_observations() {
    let input = input();
    let snapshot = InventorySnapshot {
        stacks: input.stacks.clone(),
        plug_set_entries: input.plug_set_entries.clone(),
        metadata: SnapshotMetadata {
            membership_id: input.membership_id.clone(),
            membership_type: input.membership_type,
            definition_version: input.definition_version.clone(),
            checked_at: input.checked_at,
            source: input.source,
            revision: SyncRevision::new(4),
        },
        characters: input.characters.clone(),
        items: input.items.clone(),
    };
    assert_eq!(
        InventorySnapshotInput::with_metadata(&snapshot.clone()),
        input
    );
}

#[test]
fn stack_snapshot_equality_is_a_sorted_multiset() {
    let original = input();
    let mut permuted = original.clone();
    permuted.stacks.reverse();
    permuted.stacks.rotate_left(1);
    permuted.checked_at = Timestamp::new(201);
    assert!(original.content_eq(&permuted));
    for replacement in [
        stack(StackContainer::ProfileInventory, 7, 8, 4),
        stack(StackContainer::ProfileInventory, 9, 8, 2),
        stack(StackContainer::ProfileInventory, 7, 9, 2),
        stack(StackContainer::ProfileCurrencies, 7, 8, 2),
    ] {
        let mut changed = original.clone();
        changed.stacks[0] = replacement;
        assert!(!original.content_eq(&changed));
    }
    let mut changed = original.clone();
    changed.stacks.pop();
    assert!(!original.content_eq(&changed));
    changed.stacks.push(original.stacks[1].clone());
    assert!(!original.content_eq(&changed));
}

#[test]
fn stack_snapshot_owners_must_belong_to_the_projection() {
    let mut candidate = input();
    assert!(candidate.is_structurally_valid());
    candidate.characters.push(Character {
        id: CharacterId::new("123"),
        name: None,
        class: None,
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    });
    for container in [
        StackContainer::CharacterInventory(CharacterId::new("123")),
        StackContainer::Postmaster(CharacterId::new("123")),
    ] {
        candidate.stacks = vec![stack(container, 7, 8, 1)];
        assert!(candidate.is_structurally_valid());
        let mut missing_owner = candidate.clone();
        missing_owner.characters.clear();
        assert!(!missing_owner.is_structurally_valid());
    }
    candidate.characters.push(candidate.characters[0].clone());
    assert!(!candidate.is_structurally_valid());
}
