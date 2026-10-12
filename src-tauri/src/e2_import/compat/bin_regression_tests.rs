use super::test_support::{compile, document, section, text};
use crate::e2_import::{E2RawSection, E2RawValue};

fn odd_object_scene(y: i64) -> E2RawSection {
    section(
        "BinEditSET",
        &[
            (".(0)(1)(2)isDefault", E2RawValue::Bool(true)),
            (".(0)(1)(2)posx", E2RawValue::Integer(25)),
            (".(0)(1)(2)posy", E2RawValue::Integer(y)),
            (".(0)(1)(2)sizew", E2RawValue::Integer(83)),
            (".(0)(1)(2)sizeh", E2RawValue::Integer(12)),
        ],
    )
}

#[test]
fn odd_object_y_uses_original_y_for_both_directions() {
    let decrease_doc = document(vec![odd_object_scene(50)]);
    let decrease_plan = compile(&decrease_doc, &[], Some(0)).expect("decrease odd object y");
    let decrease = text(&decrease_plan, "src/e2/bin_compat.eps");
    assert!(decrease.contains("f_epdread_epd(264099) + 65, Add, 196608"));
    assert!(decrease.contains("f_epdread_epd(264099) + 66, SetTo, 7012402"));
    assert!(decrease.contains("f_epdread_epd(264099) + 67, Subtract, 14"));

    let increase_doc = document(vec![odd_object_scene(70)]);
    let increase_plan = compile(&increase_doc, &[], Some(0)).expect("increase odd object y");
    let increase = text(&increase_plan, "src/e2/bin_compat.eps");
    assert!(increase.contains("f_epdread_epd(264099) + 66, SetTo, 7012422"));
    assert!(increase.contains("f_epdread_epd(264099) + 67, Add, 6"));
}
