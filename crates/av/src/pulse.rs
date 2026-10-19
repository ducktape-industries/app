//! A sound server's cards, for a Bluetooth headset whose microphone needs its headset
//! profile: listed while it plays music, the headset profile asked for while its microphone
//! is open, the profile it had put back once it closes.

use pulseaudio::protocol;
use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::net::UnixStream;

pub(crate) const CARD: &str = "pulseaudio-card:";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Card {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub bluetooth: bool,
    pub active: Option<String>,
    pub profiles: Vec<Profile>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Profile {
    pub name: String,
    pub available: bool,
    pub sources: u32,
    pub priority: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Source {
    pub name: String,
    pub card: Option<u32>,
    pub monitor: bool,
}

pub(crate) trait Server {
    fn cards(&mut self) -> Result<Vec<Card>, String>;
    fn sources(&mut self) -> Result<Vec<Source>, String>;
    fn set_profile(&mut self, card: &str, profile: &str) -> Result<(), String>;
    fn moved(&mut self) -> Result<(), String>;
}

pub(crate) fn headset(card: &Card) -> Option<&str> {
    card.profiles
        .iter()
        .filter(|profile| profile.available && profile.sources > 0)
        .max_by_key(|profile| profile.priority)
        .map(|profile| profile.name.as_str())
}

fn source_of(card: &Card, sources: &[Source]) -> Option<String> {
    sources
        .iter()
        .find(|source| !source.monitor && source.card == Some(card.index))
        .map(|source| source.name.clone())
}

pub(crate) fn silent_headsets(cards: &[Card], sources: &[Source]) -> Vec<(String, String)> {
    cards
        .iter()
        .filter(|card| card.bluetooth && source_of(card, sources).is_none())
        .filter(|card| headset(card).is_some())
        .map(|card| (format!("{CARD}{}", card.name), card.description.clone()))
        .collect()
}

pub(crate) struct Switched {
    pub source: String,
    pub back: Option<(String, String)>,
}

pub(crate) fn switch(server: &mut impl Server, id: &str) -> Result<Switched, String> {
    let name = id.strip_prefix(CARD).ok_or("not a headset")?;
    let card = server
        .cards()?
        .into_iter()
        .find(|card| card.bluetooth && card.name == name)
        .ok_or("the headset is gone")?;
    if let Some(source) = source_of(&card, &server.sources()?) {
        return Ok(Switched { source, back: None });
    }
    let profile = headset(&card).ok_or("the headset offers no microphone")?;
    server
        .set_profile(&card.name, profile)
        .map_err(|error| format!("the headset didn't switch to its microphone: {error}"))?;
    let back = card
        .active
        .clone()
        .map(|active| (card.name.clone(), active));
    let found = loop {
        match server.cards() {
            Ok(cards) if !cards.iter().any(|listed| listed.index == card.index) => {
                break Err("the headset went away".to_owned());
            }
            Ok(_) => {}
            Err(error) => break Err(error),
        }
        match server.sources().map(|sources| source_of(&card, &sources)) {
            Ok(Some(source)) => break Ok(source),
            Ok(None) => {}
            Err(error) => break Err(error),
        }
        if let Err(error) = server.moved() {
            break Err(error);
        }
    };
    match found {
        Ok(source) => Ok(Switched { source, back }),
        Err(error) => {
            if let Some((card, profile)) = &back {
                let _ = server.set_profile(card, profile);
            }
            Err(error)
        }
    }
}

pub(crate) struct Pulse {
    commands: BufReader<UnixStream>,
    events: Option<BufReader<UnixStream>>,
    version: u16,
    seq: u32,
}

fn connected() -> Result<(BufReader<UnixStream>, u16), String> {
    let said = |error: &dyn std::fmt::Display| error.to_string();
    let path = pulseaudio::socket_path_from_env().ok_or("no sound server")?;
    let mut socket = BufReader::new(UnixStream::connect(path).map_err(|error| said(&error))?);
    let cookie = pulseaudio::cookie_path_from_env()
        .and_then(|path| std::fs::read(path).ok())
        .unwrap_or_default();
    let auth = protocol::AuthParams {
        version: protocol::MAX_VERSION,
        supports_shm: false,
        supports_memfd: false,
        cookie,
    };
    let auth = protocol::Command::Auth(auth);
    protocol::write_command_message(socket.get_mut(), 0, &auth, protocol::MAX_VERSION)
        .map_err(|error| said(&error))?;
    let (_, reply) =
        protocol::read_reply_message::<protocol::AuthReply>(&mut socket, protocol::MAX_VERSION)
            .map_err(|error| said(&error))?;
    let version = protocol::MAX_VERSION.min(reply.version);
    let mut props = protocol::Props::new();
    props.set(
        protocol::Prop::ApplicationName,
        CString::new("Ducktape").expect("no NUL"),
    );
    let name = protocol::Command::SetClientName(props);
    protocol::write_command_message(socket.get_mut(), 1, &name, version)
        .map_err(|error| said(&error))?;
    protocol::read_reply_message::<protocol::SetClientNameReply>(&mut socket, version)
        .map_err(|error| said(&error))?;
    Ok((socket, version))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.strip_suffix(&[0]).unwrap_or(bytes)).into_owned()
}

impl Pulse {
    pub(crate) fn connect() -> Result<Self, String> {
        let (commands, version) = connected()?;
        Ok(Self {
            commands,
            events: None,
            version,
            seq: 1,
        })
    }

    fn ask<R: protocol::CommandReply>(&mut self, command: protocol::Command) -> Result<R, String> {
        self.seq += 1;
        protocol::write_command_message(self.commands.get_mut(), self.seq, &command, self.version)
            .map_err(|error| error.to_string())?;
        protocol::read_reply_message::<R>(&mut self.commands, self.version)
            .map(|(_, reply)| reply)
            .map_err(|error| error.to_string())
    }
}

impl Server for Pulse {
    fn cards(&mut self) -> Result<Vec<Card>, String> {
        let cards = self.ask::<protocol::CardInfoList>(protocol::Command::GetCardInfoList)?;
        Ok(cards
            .into_iter()
            .map(|card| Card {
                index: card.index,
                name: card.name.to_string_lossy().into_owned(),
                description: card
                    .props
                    .get(protocol::Prop::DeviceDescription)
                    .map_or_else(|| card.name.to_string_lossy().into_owned(), text),
                bluetooth: card
                    .props
                    .get(protocol::Prop::DeviceBus)
                    .is_some_and(|bus| text(bus) == "bluetooth"),
                active: card
                    .active_profile
                    .map(|active| active.to_string_lossy().into_owned()),
                profiles: card
                    .profiles
                    .into_iter()
                    .map(|profile| Profile {
                        name: profile.name.to_string_lossy().into_owned(),
                        available: profile.available != 0,
                        sources: profile.num_sources,
                        priority: profile.priority,
                    })
                    .collect(),
            })
            .collect())
    }

    fn sources(&mut self) -> Result<Vec<Source>, String> {
        let sources = self.ask::<protocol::SourceInfoList>(protocol::Command::GetSourceInfoList)?;
        Ok(sources
            .into_iter()
            .map(|source| Source {
                name: source.name.to_string_lossy().into_owned(),
                card: source.card_index,
                monitor: source.monitor_of_sink_index.is_some(),
            })
            .collect())
    }

    fn set_profile(&mut self, card: &str, profile: &str) -> Result<(), String> {
        if self.events.is_none() {
            let (mut events, version) = connected()?;
            let subscribe = protocol::Command::Subscribe(
                protocol::SubscriptionMask::SOURCE | protocol::SubscriptionMask::CARD,
            );
            protocol::write_command_message(events.get_mut(), 2, &subscribe, version)
                .map_err(|error| error.to_string())?;
            protocol::read_ack_message(&mut events).map_err(|error| error.to_string())?;
            self.events = Some(events);
        }
        let named = |text: &str| CString::new(text).map_err(|error| error.to_string());
        let set = protocol::Command::SetCardProfile(protocol::SetCardProfileParams {
            card_index: None,
            card_name: Some(named(card)?),
            profile_name: named(profile)?,
        });
        self.seq += 1;
        protocol::write_command_message(self.commands.get_mut(), self.seq, &set, self.version)
            .map_err(|error| error.to_string())?;
        protocol::read_ack_message(&mut self.commands)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn moved(&mut self) -> Result<(), String> {
        use protocol::SubscriptionEventFacility as Facility;
        use protocol::SubscriptionEventType as Type;
        let events = self.events.as_mut().ok_or("not listening")?;
        loop {
            let (_, event) = protocol::read_command_message(events, self.version)
                .map_err(|error| error.to_string())?;
            if let protocol::Command::SubscribeEvent(event) = event
                && matches!(
                    (event.event_facility, event.event_type),
                    (Facility::Source, Type::New | Type::Removed) | (Facility::Card, Type::Removed)
                )
            {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Graph {
        cards: Vec<Card>,
        sources: Vec<Source>,
        asked: Vec<(String, String)>,
        refuse: bool,
        leaves: bool,
    }

    impl Server for Graph {
        fn cards(&mut self) -> Result<Vec<Card>, String> {
            Ok(self.cards.clone())
        }

        fn sources(&mut self) -> Result<Vec<Source>, String> {
            Ok(self.sources.clone())
        }

        fn set_profile(&mut self, card: &str, profile: &str) -> Result<(), String> {
            self.asked.push((card.to_owned(), profile.to_owned()));
            if self.refuse {
                return Err("NoEntity".into());
            }
            for listed in &mut self.cards {
                if listed.name == card {
                    listed.active = Some(profile.to_owned());
                }
            }
            Ok(())
        }

        fn moved(&mut self) -> Result<(), String> {
            if self.leaves {
                self.cards.retain(|card| !card.bluetooth);
                self.sources.retain(|source| source.card != Some(7));
                return Ok(());
            }
            let headset = self.cards.iter().find(|card| card.bluetooth).unwrap();
            assert_eq!(headset.active.as_deref(), Some("headset-head-unit"));
            self.sources.push(Source {
                name: "bluez_input.AA_BB_CC_DD_EE_FF.0".into(),
                card: Some(headset.index),
                monitor: false,
            });
            Ok(())
        }
    }

    fn profile(name: &str, sources: u32, priority: u32) -> Profile {
        Profile {
            name: name.into(),
            available: true,
            sources,
            priority,
        }
    }

    fn music_headset(refuse: bool) -> Graph {
        let headset = Card {
            index: 7,
            name: "bluez_card.AA_BB_CC_DD_EE_FF".into(),
            description: "WH-1000XM4".into(),
            bluetooth: true,
            active: Some("a2dp-sink".into()),
            profiles: vec![
                profile("off", 0, 0),
                profile("a2dp-sink", 0, 40),
                profile("headset-head-unit-cvsd", 1, 20),
                profile("headset-head-unit", 1, 30),
            ],
        };
        let built_in = Card {
            index: 1,
            name: "alsa_card.pci-0000_00_1f.3".into(),
            description: "Built-in Audio".into(),
            bluetooth: false,
            active: Some("output:analog-stereo+input:analog-stereo".into()),
            profiles: vec![profile("output:analog-stereo+input:analog-stereo", 1, 65)],
        };
        let sources = vec![
            Source {
                name: "alsa_input.pci-0000_00_1f.3.analog-stereo".into(),
                card: Some(1),
                monitor: false,
            },
            Source {
                name: "bluez_output.AA_BB_CC_DD_EE_FF.1.monitor".into(),
                card: Some(7),
                monitor: true,
            },
        ];
        Graph {
            cards: vec![built_in, headset],
            sources,
            asked: Vec::new(),
            refuse,
            leaves: false,
        }
    }

    #[test]
    fn a_headset_playing_music_lists_its_microphone_and_opening_it_asks_the_headset_profile() {
        let mut graph = music_headset(false);
        assert_eq!(
            silent_headsets(&graph.cards, &graph.sources),
            [(
                "pulseaudio-card:bluez_card.AA_BB_CC_DD_EE_FF".to_owned(),
                "WH-1000XM4".to_owned()
            )]
        );
        let switched = switch(&mut graph, "pulseaudio-card:bluez_card.AA_BB_CC_DD_EE_FF").unwrap();
        assert_eq!(
            graph.asked,
            [(
                "bluez_card.AA_BB_CC_DD_EE_FF".to_owned(),
                "headset-head-unit".to_owned()
            )]
        );
        assert_eq!(switched.source, "bluez_input.AA_BB_CC_DD_EE_FF.0");
        assert_eq!(
            switched.back,
            Some((
                "bluez_card.AA_BB_CC_DD_EE_FF".to_owned(),
                "a2dp-sink".to_owned()
            ))
        );
        assert!(silent_headsets(&graph.cards, &graph.sources).is_empty());
        let again = switch(&mut graph, "pulseaudio-card:bluez_card.AA_BB_CC_DD_EE_FF").unwrap();
        assert_eq!(again.back, None, "already a headset: nothing to put back");
        assert_eq!(graph.asked.len(), 1);
    }

    #[test]
    fn a_headset_that_goes_away_while_it_switches_ends_the_wait_and_says_so() {
        let mut graph = music_headset(false);
        graph.leaves = true;
        let gone = switch(&mut graph, "pulseaudio-card:bluez_card.AA_BB_CC_DD_EE_FF");
        assert_eq!(gone.err().as_deref(), Some("the headset went away"));
        let card = || "bluez_card.AA_BB_CC_DD_EE_FF".to_owned();
        assert_eq!(
            graph.asked,
            [
                (card(), "headset-head-unit".to_owned()),
                (card(), "a2dp-sink".to_owned())
            ],
            "the profile it had is asked back"
        );
    }

    #[test]
    fn a_refused_switch_says_so_and_a_card_not_listed_is_not_switched() {
        let mut graph = music_headset(true);
        let refused = switch(&mut graph, "pulseaudio-card:bluez_card.AA_BB_CC_DD_EE_FF");
        assert_eq!(
            refused.err().as_deref(),
            Some("the headset didn't switch to its microphone: NoEntity")
        );
        let mut graph = music_headset(false);
        for id in [
            "pulseaudio-card:alsa_card.pci-0000_00_1f.3",
            "pulseaudio-card:bluez_card.00_00_00_00_00_00",
            "bluez_card.AA_BB_CC_DD_EE_FF",
        ] {
            assert!(switch(&mut graph, id).is_err(), "{id}");
        }
        assert!(graph.asked.is_empty());
    }
}
