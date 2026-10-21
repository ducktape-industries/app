//! The owners the runtime holds, for `crate::memory`: each seat's view and
//! pictures, each notifier, and the code the runner keeps compiled.

use super::*;
use crate::memory::{Group, Owned};

pub(crate) fn owners() -> Vec<Owned> {
    let mut owners = Vec::new();
    let mut named: HashMap<[u8; 32], String> = HashMap::new();
    let seats: Vec<_> = networks()
        .iter()
        .flat_map(|network| {
            lock(&network.seats)
                .iter()
                .map(|(key, seat)| (*key, seat.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    for ((program, instance), seat) in seats {
        let seat = lock(&seat);
        match &seat.slot {
            Slot::Ready(view) => {
                let name = format!("{program}/{instance}");
                owners.push(Owned {
                    group: Group::View,
                    name: name.clone(),
                    bytes: view.memory_bytes() as u64,
                    resident: view.memory_resident(),
                    clean: false,
                });
                owners.push(Owned {
                    group: Group::Pictures,
                    name,
                    bytes: view.pictures.bytes(),
                    resident: None,
                    clean: false,
                });
                if let Some(hash) = view.hash {
                    named.entry(hash).or_insert_with(|| program.to_owned());
                }
            }
            Slot::Compiled(code) => {
                named.entry(code.hash).or_insert_with(|| program.to_owned());
            }
            _ => {}
        }
    }
    for (owned, code) in crate::memory::tracked() {
        if let Some(hash) = code {
            named.entry(hash).or_insert_with(|| owned.name.clone());
        }
        owners.push(owned);
    }
    for held in crate::contract::runner().code_memory() {
        let short = noded_client::hex::hex_encode(&held.hash[..4]);
        let name = match (named.get(&held.hash), held.describe) {
            (Some(program), false) => program.clone(),
            (Some(program), true) => format!("{program} describe"),
            (None, true) => format!("describe {short}"),
            (None, false) => format!("code {short}"),
        };
        owners.push(Owned {
            group: Group::Code,
            name,
            bytes: held.mapped,
            resident: held.resident,
            clean: true,
        });
    }
    owners
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_networks_views_are_owners() {
        let network = Network::new(false);
        let seat = Mounted::seat();
        lock(&seat).slot = Slot::Ready(Box::new(crate::runtime::kernel::tests::view()));
        lock(&network.seats).insert(("owners-tests-view", 7), seat);
        let listed = owners()
            .iter()
            .any(|owned| owned.group == Group::View && owned.name == "owners-tests-view/7");
        lock(&network.seats).clear();
        assert!(
            listed,
            "a view on another network than the one in hand is no owner"
        );
    }
}
