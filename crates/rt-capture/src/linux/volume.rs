//! The volume of a playback stream: its node's `channelVolumes` (in its `Props` param), which the stream applies to
//! its audio, before it reaches the sink or our capture. Ducking scales all channels alike, so a stream keeps its
//! balance, and never touches the stream's mute. WirePlumber remembers a stream's volume for the next time the app
//! plays, so a duck we don't undo stays; see the duck journal.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Cursor;
use std::time::{Duration, Instant};

use pipewire::node::Node;
use pipewire::spa;
use spa::param::ParamType;
use spa::pod::deserialize::PodDeserializer;
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Property, Value, ValueArray};
use spa::utils::{Id, SpaTypes};

use crate::before_duck::Session;
use crate::duck_journal::{self, JournalEntry};

/// How long a volume we set may take to be reported back before a report that doesn't match it counts anyway.
const ECHO_TIMEOUT: Duration = Duration::from_secs(1);

/// The volume of a node as far as we know it.
#[derive(Debug, Default)]
pub(super) struct NodeVolume {
    /// `channelVolumes`, once the node reported them.
    channels: Option<Vec<f32>>,
    /// Levels we set that the node hasn't reported back yet, oldest first, with when they were set.
    pending: VecDeque<(f32, Instant)>,
}

impl NodeVolume {
    /// The node's volume: its loudest channel. `None` until the node reported its volume.
    pub fn level(&self) -> Option<f32> {
        self.channels.as_deref().map(loudest)
    }

    /// The node reported `channels`. Every level we set up to that one has arrived.
    pub fn reported(&mut self, channels: Vec<f32>) {
        let level = loudest(&channels);
        if let Some(i) = self.pending.iter().position(|&(set, _)| same(set, level)) {
            self.pending.drain(..=i);
        }
        self.channels = Some(channels);
    }

    /// Whether the reported volume comes after every level we set (or those got lost), so a level that differs from
    /// ours was set by someone else.
    pub fn settled(&self) -> bool {
        self.pending.front().is_none_or(|&(_, at)| at.elapsed() >= ECHO_TIMEOUT)
    }

    /// The channel volumes for `level`: the current ones scaled so the loudest is `level`.
    pub fn scaled(&self, level: f32) -> Vec<f32> {
        match self.channels.as_deref() {
            Some(channels) if loudest(channels) > 0.0 => {
                let scale = level / loudest(channels);
                channels.iter().map(|c| c * scale).collect()
            }
            Some(channels) if !channels.is_empty() => vec![level; channels.len()],
            _ => vec![level; 2],
        }
    }

    /// We set `level`; the node reports it back later.
    fn sent(&mut self, level: f32) {
        if self.pending.len() >= 64 {
            self.pending.pop_front();
        }
        self.pending.push_back((level, Instant::now()));
    }
}

fn loudest(channels: &[f32]) -> f32 {
    channels.iter().copied().fold(0.0, f32::max)
}

fn same(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-4
}

/// Whether the user (or another app) changed the volume of a node we ducked as `entry` says: it is settled at a level
/// that isn't the one we set. Its volume is then theirs, and restoring leaves it alone.
pub(super) fn changed_by_user(entry: &JournalEntry, volume: &NodeVolume) -> bool {
    volume.settled()
        && volume
            .level()
            .is_some_and(|level| !duck_journal::should_restore(entry, level))
}

/// A node being ducked, as [`BeforeDuck`](crate::before_duck::BeforeDuck) sees it.
pub(super) struct NodeSession<'a> {
    /// The node's `object.serial`, which unlike its id is never reused.
    pub id: String,
    /// See [`identifier`].
    pub identifier: String,
    pub node: &'a Node,
    pub volume: &'a RefCell<NodeVolume>,
}

impl Session for NodeSession<'_> {
    fn id(&self) -> &str {
        &self.id
    }

    fn identity(&self) -> Option<(String, f32)> {
        Some((self.identifier.clone(), self.volume.borrow().level()?))
    }

    fn set_volume(&self, level: f32) -> bool {
        let channels = self.volume.borrow().scaled(level);
        match props(&channels) {
            Some(pod) => {
                self.node
                    .set_param(ParamType::Props, 0, Pod::from_bytes(&pod).expect("a serialized pod"));
                self.volume.borrow_mut().sent(level);
                true
            }
            None => false,
        }
    }
}

/// What the duck journal calls a stream, so a later run finds it again, also after the app restarted: the key
/// WirePlumber remembers its volume under (`formKey` in its `state-stream.lua`), from the first of these node
/// properties it has. Every stream with that key gets the volume WirePlumber remembers, so after a crash those are
/// the streams that come back ducked.
pub(super) fn identifier<'a>(prop: impl Fn(&str) -> Option<&'a str>) -> Option<String> {
    const KEYS: [&str; 5] = [
        "media.role",
        "application.id",
        "application.name",
        "media.name",
        "node.name",
    ];
    KEYS.iter()
        .find_map(|&key| Some((key, prop(key).filter(|v| !v.is_empty())?)))
        .map(|(key, value)| format!("Output/Audio:{key}:{value}"))
}

/// A `Props` param that sets `channelVolumes`, serialized.
pub(super) fn props(channels: &[f32]) -> Option<Vec<u8>> {
    let object = Value::Object(Object {
        type_: SpaTypes::ObjectParamProps.as_raw(),
        id: ParamType::Props.as_raw(),
        properties: vec![Property::new(
            spa::sys::SPA_PROP_channelVolumes,
            Value::ValueArray(ValueArray::Float(channels.to_vec())),
        )],
    });
    let (cursor, _) = PodSerializer::serialize(Cursor::new(Vec::new()), &object).ok()?;
    Some(cursor.into_inner())
}

/// The `channelVolumes` of a `Props` param.
pub(super) fn channel_volumes(props: &Pod) -> Option<Vec<f32>> {
    let object = props.as_object().ok()?;
    let value = object.find_prop(Id(spa::sys::SPA_PROP_channelVolumes))?.value();
    // A pod inside an object isn't padded to 8 bytes, which the deserializer needs: a one-channel array (12 bytes of
    // body) would fail.
    let mut bytes = value.as_bytes().to_vec();
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    match PodDeserializer::deserialize_any_from(&bytes).ok()?.1 {
        Value::ValueArray(ValueArray::Float(channels)) if !channels.is_empty() => Some(channels),
        _ => None,
    }
}

/// What to do with a duck journal from an earlier run, given the playback streams there are now as (identifier,
/// level): the streams to give their volume from before ducking (index and level), and the entries no stream has yet,
/// because their app isn't playing. A stream whose level isn't the ducked one any more was changed by hand and is left
/// alone; its entry is done all the same.
pub(super) fn recover(journal: Vec<JournalEntry>, streams: &[(String, f32)]) -> (Vec<(usize, f32)>, Vec<JournalEntry>) {
    let mut restore = Vec::new();
    let mut pending = Vec::new();
    for entry in journal {
        let mut seen = false;
        for (i, (identifier, level)) in streams.iter().enumerate() {
            if *identifier == entry.identifier {
                seen = true;
                if duck_journal::should_restore(&entry, *level) {
                    restore.push((i, entry.original));
                }
            }
        }
        if !seen {
            pending.push(entry);
        }
    }
    (restore, pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(identifier: &str, original: f32, ducked: f32) -> JournalEntry {
        JournalEntry {
            identifier: identifier.into(),
            original,
            ducked,
        }
    }

    #[test]
    fn props_carry_channel_volumes_and_nothing_else() {
        let pod = props(&[0.5, 0.25]).unwrap();
        let pod = Pod::from_bytes(&pod).unwrap();
        assert_eq!(channel_volumes(pod), Some(vec![0.5, 0.25]));
        // A mono stream has one channel.
        let mono = props(&[0.5]).unwrap();
        assert_eq!(channel_volumes(Pod::from_bytes(&mono).unwrap()), Some(vec![0.5]));
        let object = pod.as_object().unwrap();
        assert_eq!(object.props().count(), 1, "mute and the master volume are left alone");
    }

    #[test]
    fn ducking_keeps_the_balance() {
        let mut volume = NodeVolume::default();
        assert_eq!(volume.level(), None);
        volume.reported(vec![0.8, 0.4]);
        assert_eq!(volume.level(), Some(0.8));
        assert_eq!(volume.scaled(0.2), [0.2, 0.1]);
        // A silent node gets the level on every channel.
        volume.reported(vec![0.0, 0.0]);
        assert_eq!(volume.scaled(0.5), [0.5, 0.5]);
    }

    #[test]
    fn a_volume_is_unsettled_until_the_node_reports_what_we_set() {
        let mut volume = NodeVolume::default();
        volume.reported(vec![1.0, 1.0]);
        assert!(volume.settled());
        volume.sent(0.5);
        volume.sent(0.25);
        assert!(!volume.settled());
        // Reports can skip levels: the latest one covers those before it.
        volume.reported(vec![0.25, 0.25]);
        assert!(volume.settled());
        // A report of something else while ours is under way says nothing yet.
        volume.sent(0.2);
        volume.reported(vec![0.7, 0.7]);
        assert!(!volume.settled());
    }

    #[test]
    fn a_volume_changed_during_a_duck_belongs_to_the_user() {
        let ducked = entry("Firefox", 0.8, 0.2);
        let mut volume = NodeVolume::default();
        volume.reported(vec![0.2, 0.2]);
        assert!(!changed_by_user(&ducked, &volume));
        volume.reported(vec![0.6, 0.6]);
        assert!(changed_by_user(&ducked, &volume));
        // Our own fade, not reported back yet, isn't mistaken for the user's.
        volume.sent(0.2);
        assert!(!changed_by_user(&ducked, &volume));
    }

    #[test]
    fn journal_identifiers_are_wireplumbers_restore_keys() {
        let props = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
        };
        let firefox = props(&[
            ("application.name", "Firefox"),
            ("media.name", "AudioStream"),
            ("node.name", "Firefox"),
            ("application.process.binary", "firefox-esr"),
        ]);
        assert_eq!(
            identifier(firefox).as_deref(),
            Some("Output/Audio:application.name:Firefox")
        );
        // The role comes first: WirePlumber gives every stream of a role the same volume.
        let music = props(&[("media.role", "Music"), ("application.name", "pw-cat")]);
        assert_eq!(identifier(music).as_deref(), Some("Output/Audio:media.role:Music"));
        let flatpak = props(&[
            ("application.id", "org.mozilla.firefox"),
            ("application.name", "Firefox"),
        ]);
        assert_eq!(
            identifier(flatpak).as_deref(),
            Some("Output/Audio:application.id:org.mozilla.firefox")
        );
        assert_eq!(
            identifier(props(&[("media.role", ""), ("node.name", "n")])).as_deref(),
            Some("Output/Audio:node.name:n")
        );
        assert_eq!(identifier(props(&[("application.process.binary", "x")])), None);
    }

    #[test]
    fn recovery_restores_streams_still_at_the_ducked_volume() {
        let journal = vec![
            entry("Firefox", 0.8, 0.16),
            entry("Discord", 1.0, 0.2),
            entry("Spotify", 0.5, 0.1),
        ];
        let streams = [
            ("Firefox".to_owned(), 0.16),
            ("Firefox".to_owned(), 0.16),
            // Changed by hand since: left alone.
            ("Discord".to_owned(), 0.9),
            ("Other".to_owned(), 0.2),
        ];
        let (restore, pending) = recover(journal, &streams);
        assert_eq!(restore, [(0, 0.8), (1, 0.8)]);
        // Spotify isn't playing: it waits for the next time.
        assert_eq!(pending, [entry("Spotify", 0.5, 0.1)]);
    }
}
