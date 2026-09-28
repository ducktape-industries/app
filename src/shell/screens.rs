//! Names as the bar shows them: a raw program id read as a label, and an
//! account name's initials. (screens_tests.rs pins `screens::initials`;
//! the file keeps its name until that test moves.)

/// A raw program id read as a tab label before its manifest arrives, or
/// after it failed to: hyphens to spaces, first letter capitalised —
/// "module-registry" reads "Module registry" instead of flashing the kebab
/// id and then the manifest name.
pub(super) fn prettify(id: &str) -> String {
    let spaced = id.replace('-', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// The avatar's letters for an account `name`: the first letter of its
/// first two words ("Ada Lovelace" → "AL", "ada" → "A"), "?" for none.
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
