//! A field its form refuses empty says it is required, and one the screen
//! draws with an error says it is invalid and, as its description, why
//! (AX-108, AX-109).
use super::*;
use serde_json::json;

#[gpui_kit::test]
fn a_field_drawn_with_an_error_says_so_and_why(cx: &mut TestAppContext) {
    let cases = [
        (
            "connect-error-and-recent",
            "TextInput",
            "Node address",
            Some("no route to host"),
        ),
        (
            "sign-in-old-password-error",
            "PasswordInput",
            "Password",
            Some("wrong password"),
        ),
        ("connect", "TextInput", "Node address", None),
        ("account-step", "TextInput", "Account name", None),
    ];
    let mut matrix = gate::matrix();
    for (screen, role, name, error) in cases {
        let at = matrix.iter().position(|(s, ..)| *s == screen).unwrap();
        let (_, _, build) = matrix.swap_remove(at);
        let (_view, mut native) = open(build(), cx);
        let nodes = native.update(draw);
        let field = find(&nodes, role, name);
        assert_eq!(field["required"], true, "{screen}: {field}");
        let (invalid, why) = match error {
            Some(error) => (json!("true"), json!(error)),
            None => (json!(null), json!(null)),
        };
        assert_eq!(field["invalid"], invalid, "{screen}: {field}");
        assert_eq!(field["description"], why, "{screen}: {field}");
    }
}
