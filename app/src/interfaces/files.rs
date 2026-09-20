use duck_address::{Address, ChainId, Refused};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAddress {
    pub path: Vec<String>,
}

impl TryFrom<&Address> for FileAddress {
    type Error = Refused;
    fn try_from(address: &Address) -> Result<Self, Self::Error> {
        if address.module != "files" {
            return Err(Refused::new(
                "invalid_input",
                format!("A file address names the module `{}`.", address.module),
            ));
        }
        if address.path.is_empty() {
            return Err(Refused::new(
                "invalid_input",
                "A file address needs at least one path segment.",
            ));
        }
        if address.path.len() > 128
            || address
                .path
                .iter()
                .map(|segment| segment.len() + 1)
                .sum::<usize>()
                > 4096
        {
            return Err(Refused::new(
                "invalid_input",
                "A file address exceeds the 128-segment or 4096-byte path limit.",
            ));
        }
        if address
            .path
            .iter()
            .any(|segment| segment.nfc().ne(segment.chars()))
        {
            return Err(Refused::new(
                "invalid_input",
                "A file address names a path that is not NFC-normalized.",
            ));
        }
        if address.path.iter().any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains(['/', '\0'])
                || segment.len() > 255
        }) {
            return Err(Refused::new(
                "invalid_input",
                "A file address names a non-canonical duckfs path.",
            ));
        }
        Ok(Self {
            path: address.path.clone(),
        })
    }
}

impl FileAddress {
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        let address = Address::new(chain, "files", self.path.clone())?;
        Self::try_from(&address)?;
        Ok(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_address_preserves_path_byte_and_depth_bounds() {
        let mut address = Address::parse("duck://testnet-abcd/files/a").unwrap();
        address.path = vec!["a".into(); 128];
        assert!(FileAddress::try_from(&address).is_ok());
        address.path.push("a".into());
        assert!(FileAddress::try_from(&address).is_err());
        address.path = vec!["a".repeat(255); 16];
        assert!(FileAddress::try_from(&address).is_ok());
        address.path.push("a".into());
        assert!(FileAddress::try_from(&address).is_err());
    }
}
