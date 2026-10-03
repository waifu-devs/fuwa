//! A conversation between two people on several devices, the way the server
//! and the apps drive it: every record goes through one ordered log, and
//! every device reads it from where it joined.

use fuwa_e2ee::wire::{self, ContentType, Sender, WireFormat};
use fuwa_e2ee::{Device, Error, Processed, device_id, safety_number};

const CONVERSATION: &str = "01JZ0000000000000000000000";

fn people() -> Vec<String> {
    vec!["alice".into(), "bob".into()]
}

/// The server's side: the log, the group info, and the epoch it expects.
#[derive(Default)]
struct Log {
    records: Vec<(Vec<u8>, String)>,
    group_info: Option<Vec<u8>>,
    epoch: u64,
}

impl Log {
    /// Takes a record if it's for the current epoch, as the server does.
    fn append(&mut self, bytes: &[u8], from: &Device) -> bool {
        let header = wire::message_header(bytes).unwrap();
        assert_eq!(header.group_id, CONVERSATION.as_bytes());
        if header.epoch != self.epoch {
            return false;
        }
        if header.content_type == ContentType::Commit {
            self.epoch += 1;
        }
        self.records.push((bytes.to_vec(), from.device_id()));
        true
    }

    fn commit(&mut self, commit: &fuwa_e2ee::Commit, from: &Device) -> bool {
        if !self.append(&commit.commit, from) {
            return false;
        }
        let info = wire::group_info_header(&commit.group_info).unwrap();
        assert_eq!(info.group_id, CONVERSATION.as_bytes());
        assert_eq!(info.epoch, self.epoch);
        self.group_info = Some(commit.group_info.clone());
        true
    }
}

/// A device reading the log from where it left off.
struct Reader {
    device: Device,
    next: usize,
}

impl Reader {
    fn new(device: Device) -> Self {
        Self { device, next: 0 }
    }

    fn catch_up(&mut self, log: &Log) -> Vec<Processed> {
        let mut out = Vec::new();
        while self.next < log.records.len() {
            let (bytes, from) = &log.records[self.next];
            let own = *from == self.device.device_id();
            out.push(self.device.process(CONVERSATION, bytes, own, &people()).unwrap());
            self.next += 1;
        }
        out
    }

    fn text(&mut self, log: &Log) -> Vec<String> {
        self.catch_up(log)
            .into_iter()
            .filter_map(|p| match p {
                Processed::Message { plaintext, .. } => Some(String::from_utf8(plaintext).unwrap()),
                _ => None,
            })
            .collect()
    }
}

fn adds(device: &Device) -> (String, Vec<u8>) {
    (device.device_id(), device.key_packages(1).unwrap().remove(0))
}

#[test]
fn key_packages_say_whose_they_are() {
    let bob = Device::new("bob").unwrap();
    let package = bob.key_packages(1).unwrap().remove(0);
    let info = wire::key_package_info(&package).unwrap();
    assert_eq!(info.cipher_suite, fuwa_e2ee::CIPHER_SUITE);
    assert_eq!(info.identity, b"bob");
    assert_eq!(info.signature_key, bob.signature_key());
    assert_eq!(device_id(info.signature_key), bob.device_id());
    assert!(!info.last_resort);
    assert!(info.not_after > 1_700_000_000);

    let last = bob.last_resort_key_package().unwrap();
    assert!(wire::key_package_info(&last).unwrap().last_resort);
    assert!(wire::message_header(&package).is_err());
}

#[test]
fn two_people_on_several_devices() {
    let mut log = Log::default();
    let mut alice1 = Reader::new(Device::new("alice").unwrap());
    let mut alice2 = Reader::new(Device::new("alice").unwrap());
    let mut bob1 = Reader::new(Device::new("bob").unwrap());

    // Alice's first device starts the conversation with everyone's devices.
    alice1.device.create_group(CONVERSATION).unwrap();
    let commit =
        alice1.device.commit(CONVERSATION, &[adds(&bob1.device), adds(&alice2.device)], &[], &people()).unwrap();
    assert_eq!(commit.added.len(), 2);
    let header = wire::message_header(&commit.commit).unwrap();
    assert_eq!(
        (header.wire_format, header.epoch, header.content_type),
        (WireFormat::PrivateMessage, 0, ContentType::Commit)
    );
    assert!(wire::is_welcome(commit.welcome.as_ref().unwrap()));
    assert!(log.commit(&commit, &alice1.device));
    let welcome = commit.welcome.unwrap();

    match &alice1.catch_up(&log)[..] {
        [Processed::Commit { added, removed, .. }] => {
            assert_eq!(added.len(), 2);
            assert!(removed.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(alice1.device.members(CONVERSATION).unwrap().len(), 3);

    // The others join from the welcome and skip what came before.
    assert_eq!(bob1.device.join_from_welcome(CONVERSATION, &welcome, &people()).unwrap(), 1);
    assert_eq!(alice2.device.join_from_welcome(CONVERSATION, &welcome, &people()).unwrap(), 1);
    assert_eq!(bob1.catch_up(&log), vec![Processed::Stale]);
    alice2.catch_up(&log);

    // A message reaches every device but the one that sent it.
    let sealed = alice1.device.encrypt(CONVERSATION, b"hi bob").unwrap();
    let header = wire::message_header(&sealed).unwrap();
    assert_eq!((header.epoch, header.content_type), (1, ContentType::Application));
    assert!(!sealed.windows(6).any(|w| w == b"hi bob"));
    assert!(log.append(&sealed, &alice1.device));
    assert_eq!(bob1.text(&log), vec!["hi bob"]);
    assert_eq!(alice2.text(&log), vec!["hi bob"]);
    assert_eq!(alice1.catch_up(&log), vec![Processed::Own]);

    // Saved and restored, a device carries on where it was.
    let saved = bob1.device.save();
    bob1.device = Device::restore(&saved).unwrap();
    assert_eq!(bob1.device.user_id(), "bob");
    let sealed = bob1.device.encrypt(CONVERSATION, b"hey alice").unwrap();
    assert!(log.append(&sealed, &bob1.device));
    bob1.catch_up(&log);
    assert_eq!(alice1.text(&log), vec!["hey alice"]);
    match &alice2.catch_up(&log)[..] {
        [Processed::Message { sender, plaintext }] => {
            assert_eq!(sender.user_id, "bob");
            assert_eq!(sender.device_id, bob1.device.device_id());
            assert_eq!(plaintext, b"hey alice");
        }
        other => panic!("{other:?}"),
    }

    // Bob's new device joins by itself from the group info.
    let mut bob2 = Reader::new(Device::new("bob").unwrap());
    let join = bob2.device.join_by_itself(CONVERSATION, log.group_info.as_ref().unwrap(), &people()).unwrap();
    let header = wire::message_header(&join.commit).unwrap();
    assert_eq!(header.wire_format, WireFormat::PublicMessage);
    assert_eq!(header.sender, Some(Sender::NewMemberCommit));
    assert!(log.commit(&join, &bob2.device));
    // It's in at once; its own commit in the log is behind it.
    assert_eq!(join.added[0].device_id, bob2.device.device_id());
    bob2.next = log.records.len() - 1;
    assert_eq!(bob2.catch_up(&log), vec![Processed::Stale]);
    for reader in [&mut alice1, &mut alice2, &mut bob1] {
        match &reader.catch_up(&log)[..] {
            [Processed::Commit { by, added, .. }] => {
                assert_eq!(added.len(), 1);
                assert_eq!(added[0].device_id, bob2.device.device_id());
                assert_eq!(by.as_ref().unwrap().device_id, bob2.device.device_id());
            }
            other => panic!("{other:?}"),
        }
    }
    let sealed = bob2.device.encrypt(CONVERSATION, b"from my phone").unwrap();
    assert!(log.append(&sealed, &bob2.device));
    bob2.catch_up(&log);
    assert_eq!(alice1.text(&log), vec!["from my phone"]);

    // Everyone sees the same safety number.
    let number = |reader: &Reader| {
        let members = reader.device.members(CONVERSATION).unwrap();
        let keys = |user: &str| {
            members.iter().filter(|m| m.user_id == user).map(|m| m.signature_key.clone()).collect::<Vec<_>>()
        };
        safety_number(("alice", &keys("alice")), ("bob", &keys("bob")))
    };
    assert_eq!(number(&alice1), number(&bob2));

    // Alice signs out on her second device; the next commit removes it.
    let gone = alice2.device.device_id();
    let commit = bob1.device.commit(CONVERSATION, &[], std::slice::from_ref(&gone), &people()).unwrap();
    assert_eq!(commit.removed.len(), 1);
    assert!(log.commit(&commit, &bob1.device));
    // Bob's first device hadn't read the last message yet: it does, then its commit.
    for reader in [&mut alice1, &mut bob1, &mut bob2] {
        match reader.catch_up(&log).last() {
            Some(Processed::Commit { removed, removed_me: false, .. }) => assert_eq!(removed[0].device_id, gone),
            other => panic!("{other:?}"),
        }
    }
    assert!(matches!(alice2.catch_up(&log).last(), Some(Processed::Commit { removed_me: true, .. })));
    assert!(!alice2.device.is_member(CONVERSATION));
    let sealed = alice1.device.encrypt(CONVERSATION, b"after").unwrap();
    assert!(log.append(&sealed, &alice1.device));
    assert_eq!(bob2.text(&log), vec!["after"]);
    alice1.catch_up(&log);
    bob1.catch_up(&log);
}

#[test]
fn only_the_conversations_people_get_in() {
    let mut log = Log::default();
    let alice = Device::new("alice").unwrap();
    let bob = Device::new("bob").unwrap();
    let carol = Device::new("carol").unwrap();

    alice.create_group(CONVERSATION).unwrap();
    // A key package for someone else, or claimed for the wrong device, is refused.
    assert!(matches!(alice.commit(CONVERSATION, &[adds(&carol)], &[], &people()), Err(Error::Intruder(_))));
    let wrong = (bob.device_id(), carol.key_packages(1).unwrap().remove(0));
    assert!(matches!(alice.commit(CONVERSATION, &[wrong], &[], &people()), Err(Error::Invalid(_))));

    let commit = alice.commit(CONVERSATION, &[adds(&bob)], &[], &people()).unwrap();
    assert!(log.commit(&commit, &alice));
    alice.process(CONVERSATION, &log.records[0].0, true, &people()).unwrap();
    bob.join_from_welcome(CONVERSATION, commit.welcome.as_ref().unwrap(), &people()).unwrap();

    // A device that lets Carol in (or a server that forged it) is refused by the others.
    let everyone = vec!["alice".to_string(), "bob".to_string(), "carol".to_string()];
    let sneaky = bob.commit(CONVERSATION, &[adds(&carol)], &[], &everyone).unwrap();
    assert!(log.commit(&sneaky, &bob));
    let result = alice.process(CONVERSATION, &log.records[1].0, false, &people());
    assert!(matches!(result, Err(Error::Intruder(_))), "{result:?}");
    // Nor will Carol's device join a conversation by itself.
    assert!(matches!(carol.join_by_itself(CONVERSATION, &commit.group_info, &people()), Err(Error::Intruder(_))));
}

#[test]
fn a_commit_that_loses_the_race_gives_way() {
    let mut log = Log::default();
    let alice = Device::new("alice").unwrap();
    let bob = Device::new("bob").unwrap();
    let bob2 = Device::new("bob").unwrap();
    let alice2 = Device::new("alice").unwrap();

    alice.create_group(CONVERSATION).unwrap();
    let commit = alice.commit(CONVERSATION, &[adds(&bob)], &[], &people()).unwrap();
    assert!(log.commit(&commit, &alice));
    alice.process(CONVERSATION, &log.records[0].0, true, &people()).unwrap();
    bob.join_from_welcome(CONVERSATION, commit.welcome.as_ref().unwrap(), &people()).unwrap();

    // Both add a device at once; the server takes Alice's.
    let alices = alice.commit(CONVERSATION, &[adds(&bob2)], &[], &people()).unwrap();
    let bobs = bob.commit(CONVERSATION, &[adds(&alice2)], &[], &people()).unwrap();
    assert!(log.commit(&alices, &alice));
    assert!(!log.commit(&bobs, &bob));
    assert!(bob.has_pending_commit(CONVERSATION).unwrap());
    // Bob's device reads Alice's commit in place of its own, and stays in step.
    assert!(matches!(
        bob.process(CONVERSATION, &log.records[1].0, false, &people()).unwrap(),
        Processed::Commit { .. }
    ));
    assert!(!bob.has_pending_commit(CONVERSATION).unwrap());
    alice.process(CONVERSATION, &log.records[1].0, true, &people()).unwrap();
    assert_eq!(alice.epoch(CONVERSATION).unwrap(), bob.epoch(CONVERSATION).unwrap());

    // A message from an epoch this device hasn't reached means it missed something.
    let sealed = alice.encrypt(CONVERSATION, b"x").unwrap();
    let fresh = Device::new("bob").unwrap();
    fresh.create_group(CONVERSATION).unwrap();
    assert!(matches!(fresh.process(CONVERSATION, &sealed, false, &people()), Err(Error::Behind)));
}

#[test]
fn what_a_device_signs_can_be_checked_by_its_key_alone() {
    let mika = Device::new("mika").unwrap();
    let rin = Device::new("rin").unwrap();
    let signature = mika.sign(b"meet at noon").unwrap();
    assert!(fuwa_e2ee::verify(mika.signature_key(), b"meet at noon", &signature));
    assert!(!fuwa_e2ee::verify(mika.signature_key(), b"meet at one", &signature));
    assert!(!fuwa_e2ee::verify(rin.signature_key(), b"meet at noon", &signature));
    assert!(!fuwa_e2ee::verify(mika.signature_key(), b"meet at noon", &[0; 64]));
}
