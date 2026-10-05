use super::*;
use destiny_app::{
    CharacterId, CharacterSubclassOptions, ElementOptions, ItemHash, OptionGroup, OptionStatus,
    SubclassElement, SubclassOption, SubclassOptionsResult, SubclassPlugKind,
};

fn fixture() -> SubclassOptionsResult {
    use OptionStatus::*;
    let options = [Unlocked, Locked, Unknown]
        .into_iter()
        .enumerate()
        .map(|(i, status)| SubclassOption {
            plug_hash: ItemHash::new(i as i64),
            name: (i != 1).then(|| format!("Plug {i}")),
            description: None,
            status,
            can_insert: i == 0,
            enabled: i != 1,
        })
        .collect();
    let hunter = CharacterSubclassOptions {
        character_id: CharacterId::new("1"),
        class: Some("Hunter".into()),
        status_reliable: true,
        elements: vec![
            ElementOptions {
                element: Some(SubclassElement::Prismatic),
                groups: vec![OptionGroup {
                    kind: SubclassPlugKind::Aspect,
                    options,
                }],
            },
            ElementOptions {
                element: None,
                groups: vec![OptionGroup {
                    kind: SubclassPlugKind::Fragment,
                    options: vec![],
                }],
            },
        ],
    };
    let mut titan = hunter.clone();
    titan.character_id = CharacterId::new("2");
    titan.class = Some("Titan".into());
    titan.status_reliable = false;
    for option in &mut titan.elements[0].groups[0].options {
        option.status = Unknown;
    }
    SubclassOptionsResult {
        reference_character_id: Some(CharacterId::new("1")),
        characters: vec![hunter, titan],
    }
}

#[test]
fn option_descriptions_follow_human_locked_filter_and_json_keeps_all() {
    let mut result = fixture();
    let option = &mut result.characters[0].elements[0].groups[0].options[0];
    option.description = Some("Official option tooltip.".into());
    let value = subclass_options_json(&result);
    let options = &value["characters"][0]["elements"][0]["groups"][0]["options"];
    assert_eq!(options[0]["description"], "Official option tooltip.");
    assert!(options[1].get("description").is_none());
    assert!(
        render_subclass_options(OutputMode::Human, &result, false)
            .contains("      Plug 0:\n        Official option tooltip.")
    );
    assert!(
        !render_subclass_options(OutputMode::Human, &result, true)
            .contains("Official option tooltip.")
    );
}

#[test]
fn subclass_options_text_explains_status_and_filters_locked() {
    assert_eq!(
        render_subclass_options(OutputMode::Human, &fixture(), false),
        "Hunter (status from last-played character)\n  Prismatic\n    Aspects: Plug 0, #1 [locked], Plug 2\nTitan (status unknown: not the last-played character)\n  Prismatic\n    Aspects: Plug 0, #1, Plug 2"
    );
    assert_eq!(
        render_subclass_options(OutputMode::Human, &fixture(), true),
        "Hunter (status from last-played character)\n  Prismatic\n    Aspects: #1 [locked]\nTitan (status unknown: not the last-played character)"
    );
}

#[test]
fn subclass_options_json_preserves_all_fields() {
    let result = fixture();
    let value: Value =
        serde_json::from_str(&render_subclass_options(OutputMode::Json, &result, false)).unwrap();
    assert_eq!(value, subclass_options_json(&result));
    assert_eq!(value["kind"], "subclass_options");
    assert_eq!(value["reference_character_id"], "1");
    assert_eq!(
        value["characters"][0],
        json!({"character_id":"1", "class":"Hunter", "status_reliable":true,
        "elements":[{"element":"prismatic", "groups":[{"kind":"aspect", "options":[
            {"plug_hash":0,"name":"Plug 0","status":"unlocked","can_insert":true,"enabled":true},
            {"plug_hash":1,"name":null,"status":"locked","can_insert":false,"enabled":false},
            {"plug_hash":2,"name":"Plug 2","status":"unknown","can_insert":false,"enabled":true}
        ]}]}, {"element":null,"groups":[{"kind":"fragment","options":[]}]}]})
    );
    assert_eq!(value["characters"][1]["status_reliable"], false);
    assert_eq!(
        value["characters"][1]["elements"][0]["groups"][0]["options"][0]["status"],
        "unknown"
    );
}

#[test]
fn subclass_options_shared_labels_fallbacks_and_sanitization() {
    let mut result = fixture();
    result.characters.truncate(1);
    result.characters[0].class = None;
    let option = SubclassOption {
        plug_hash: ItemHash::new(9),
        name: Some("Name\nInjected".into()),
        description: None,
        status: OptionStatus::Unlocked,
        can_insert: true,
        enabled: true,
    };
    result.characters[0].elements = vec![ElementOptions {
        element: None,
        groups: SUBCLASS_LABELS
            .iter()
            .map(|&(kind, _)| OptionGroup {
                kind,
                options: vec![option.clone()],
            })
            .collect(),
    }];
    let text = render_subclass_options(OutputMode::Human, &result, false);
    assert!(text.starts_with("Unknown (status from last-played character)\n  Shared\n"));
    for &(_, label) in SUBCLASS_LABELS {
        assert!(text.contains(&format!("    {label}: Name?Injected")));
    }
    assert_eq!(
        render_subclass_options(OutputMode::Human, &result, true),
        "Unknown (status from last-played character)"
    );
    result.characters.clear();
    assert_eq!(
        render_subclass_options(OutputMode::Human, &result, false),
        "no characters"
    );
}
