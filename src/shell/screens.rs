//! Names as the bar shows them: a raw program id read as a label, and an
//! account name's initials. (screens_tests.rs pins `screens::initials`;
//! the file keeps its name until that test moves.)

pub(super) fn prettify(id: &str) -> String {
    let spaced = id.replace('-', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

pub(super) fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    match letters.is_empty() {
        true => "?".into(),
        false => letters,
    }
}
