use destiny_core::{ObjectiveDefinition, build_capture::crafted_date_from_objectives};

#[test]
fn crafting_date_requires_dim_complete_crafting_info() {
    let defs = [
        ObjectiveDefinition::new(1, String::new(), 0)
            .unwrap()
            .with_ui_style(Some(2)),
        ObjectiveDefinition::new(2, String::new(), 0)
            .unwrap()
            .with_ui_style(Some(3)),
        ObjectiveDefinition::new(3, String::new(), 0)
            .unwrap()
            .with_ui_style(Some(4)),
    ];
    let objectives = [(1, Some(10)), (2, Some(20)), (3, Some(1700000000))];
    assert_eq!(
        crafted_date_from_objectives(&objectives, &defs),
        Some(1700000000)
    );
    assert_eq!(crafted_date_from_objectives(&objectives[..2], &defs), None);
    assert_eq!(crafted_date_from_objectives(&objectives, &defs[..2]), None);
    assert_eq!(
        crafted_date_from_objectives(&[(1, None), (2, Some(0)), (3, Some(4))], &defs),
        None
    );
}
