use clap::Parser;
use destiny_app::{BatchEquipItem, BatchEquipResult, BatchItemStatus, ItemInstanceId, ItemSlot};
use destiny_cli::command::{Cli, Command};
use destiny_mcp::schema::EquipItemsArgs;
use destiny_output::equip_items::EquipItemsOutput;

#[test]
fn cli_mcp_batch_intent_parity_and_single_path() {
    let cli = Cli::try_parse_from([
        "ghostlink",
        "equip",
        "101",
        "Ace of Spades",
        "--character",
        "hunter",
    ])
    .unwrap();
    let Command::EquipItems(cli) = cli.command else {
        panic!("expected batch path")
    };
    let mcp = EquipItemsArgs {
        character: "hunter".into(),
        items: vec!["101".into(), "Ace of Spades".into()],
    };
    assert_eq!(cli.request().unwrap(), mcp.request().unwrap());
    let cli = Cli::try_parse_from(["ghostlink", "equip", "101", "--character", "hunter"]).unwrap();
    let Command::Equip(cli) = cli.command else {
        panic!("single-item path changed")
    };
    assert_eq!(cli.item, "101");
    assert!(
        EquipItemsArgs {
            character: "hunter".into(),
            items: vec!["101".into()],
        }
        .request()
        .is_err()
    );
}

#[test]
fn batch_output_preserves_statuses_and_fails_closed_sanitized() {
    for (status, storage, exit) in [
        (BatchItemStatus::Success, true, 0),
        (BatchItemStatus::Failure(123), true, 41),
        (BatchItemStatus::NotReported, true, 41),
        (BatchItemStatus::NoOp, true, 0),
        (BatchItemStatus::Success, false, 42),
    ] {
        let output = EquipItemsOutput::from_result(Ok(BatchEquipResult {
            items: vec![BatchEquipItem {
                item: ItemInstanceId::new("101"),
                name: "Item\nforged".into(),
                slot: ItemSlot::Kinetic,
                status,
            }],
            receipt: None,
            local_state_updated: storage,
            transfer_failed: false,
        }));
        assert_eq!(output.exit_code, exit);
        assert_eq!(
            output.value["local_state"],
            if exit != 0 {
                "refresh_pending"
            } else if status == BatchItemStatus::NoOp {
                "unchanged"
            } else {
                "updated"
            }
        );
        assert_eq!(output.value["items"][0]["name"], "Item?forged");
        assert!(!output.render_text().contains("\nforged"));
        if exit == 42 {
            assert_eq!(
                output.value["reason"],
                "local state could not be updated after the action"
            );
        }
    }
}
