//! One device's side of end-to-end encrypted conversations (feature
//! `client`): its signing key, the MLS group of every conversation it's in,
//! and sealing and opening messages.
//!
//! Each conversation is a group whose id is the conversation's id. A member
//! is a device, and its basic credential's identity is its account's id, so
//! every message says which person (and which of their devices) sent it.
//! Whoever holds a conversation only ever lets in devices of its people
//! (`allowed`), and refuses changes that would bring in anyone else.

use std::collections::BTreeMap;

use openmls::prelude::tls_codec::Deserialize as _;
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider;

use crate::device_id;

const SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519;

/// Messages are padded to a multiple of this many bytes, so their length
/// says less about what they say.
const PADDING: usize = 64;

/// How far out of order one sender's messages may arrive, and how far ahead
/// of the last one seen.
const OUT_OF_ORDER: u32 = 32;
const MAX_FORWARD: u32 = 1000;

/// What a saved device starts with, and its format.
const MAGIC: &[u8] = b"fuwa-e2ee";
const FORMAT: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input that isn't what it claims: a malformed message, or a key package
    /// for another device.
    #[error("{0}")]
    Invalid(String),
    /// The device isn't in the conversation's group (not yet, or not any more).
    #[error("this device isn't in that conversation")]
    NotMember,
    /// A change would let in a device of someone who isn't in the conversation.
    #[error("{0}")]
    Intruder(String),
    /// The message is for an epoch this device hasn't reached: it missed a commit.
    #[error("this device fell behind the conversation")]
    Behind,
    #[error("{0}")]
    Mls(String),
}

pub type Result<T> = std::result::Result<T, Error>;

fn mls(err: impl std::fmt::Display) -> Error {
    Error::Mls(err.to_string())
}

fn invalid(err: impl std::fmt::Display) -> Error {
    Error::Invalid(err.to_string())
}

/// A device in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Member {
    pub user_id: String,
    pub device_id: String,
    pub signature_key: Vec<u8>,
}

/// A commit to send: what the server stores, and what it hands the devices
/// it adds.
#[derive(Debug, Clone)]
pub struct Commit {
    pub commit: Vec<u8>,
    /// The group after the commit, for devices that join by themselves later.
    pub group_info: Vec<u8>,
    /// For the devices it adds, if any.
    pub welcome: Option<Vec<u8>>,
    pub added: Vec<Member>,
    pub removed: Vec<Member>,
}

/// What a record in a conversation turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processed {
    /// A message, and who sent it.
    Message { sender: Member, plaintext: Vec<u8> },
    /// A change in who's in the conversation. `by` is who made it (the device
    /// itself, for one joining by itself).
    Commit { by: Option<Member>, added: Vec<Member>, removed: Vec<Member>, removed_me: bool },
    /// From before this device was in the group, or already applied.
    Stale,
    /// This device's own message, which it can't open again: it keeps what it
    /// sent itself.
    Own,
}

/// A device's keys and conversations.
pub struct Device {
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    user_id: String,
}

impl Device {
    /// A new device for an account, with a new signing key.
    pub fn new(user_id: &str) -> Result<Self> {
        let provider = OpenMlsRustCrypto::default();
        let signer = SignatureKeyPair::new(SUITE.signature_algorithm()).map_err(mls)?;
        signer.store(provider.storage()).map_err(mls)?;
        Ok(Self { provider, signer, user_id: user_id.to_owned() })
    }

    /// A device as [`Device::save`] left it.
    pub fn restore(bytes: &[u8]) -> Result<Self> {
        let mut r = bytes;
        let mut take = |n: usize| -> Result<&[u8]> {
            if r.len() < n {
                return Err(Error::Invalid("the saved device is cut short".into()));
            }
            let (head, rest) = r.split_at(n);
            r = rest;
            Ok(head)
        };
        if take(MAGIC.len())? != MAGIC || take(1)?[0] != FORMAT {
            return Err(Error::Invalid("that isn't a saved device".into()));
        }
        let mut field = || -> Result<Vec<u8>> {
            let length = u32::from_be_bytes(take(4)?.try_into().expect("four bytes")) as usize;
            Ok(take(length)?.to_vec())
        };
        let user_id = String::from_utf8(field()?).map_err(invalid)?;
        let public_key = field()?;
        let count = u32::from_be_bytes(field()?.try_into().map_err(|_| invalid("bad count"))?);
        let mut values = std::collections::HashMap::with_capacity(count as usize);
        for _ in 0..count {
            let key = field()?;
            values.insert(key, field()?);
        }
        let provider = OpenMlsRustCrypto::default();
        *provider.storage().values.write().map_err(|_| invalid("storage lock"))? = values;
        let signer = SignatureKeyPair::read(provider.storage(), &public_key, SUITE.signature_algorithm())
            .ok_or_else(|| Error::Invalid("the saved device has no signing key".into()))?;
        Ok(Self { provider, signer, user_id })
    }

    /// Everything the device knows, to keep and [`Device::restore`] later.
    /// Save after every change that produced something to send, before
    /// sending it: a device that forgot it sent something would reuse keys.
    pub fn save(&self) -> Vec<u8> {
        let values = self.provider.storage().values.read().unwrap_or_else(|p| p.into_inner());
        let sorted: BTreeMap<&Vec<u8>, &Vec<u8>> = values.iter().collect();
        let mut out = Vec::new();
        out.extend(MAGIC);
        out.push(FORMAT);
        let mut field = |bytes: &[u8]| {
            out.extend((bytes.len() as u32).to_be_bytes());
            out.extend(bytes);
        };
        field(self.user_id.as_bytes());
        field(self.signer.public());
        field(&(sorted.len() as u32).to_be_bytes());
        for (key, value) in sorted {
            field(key);
            field(value);
        }
        out
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn signature_key(&self) -> &[u8] {
        self.signer.public()
    }

    pub fn device_id(&self) -> String {
        device_id(self.signer.public())
    }

    fn credential(&self) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(self.user_id.as_bytes().to_vec()).into(),
            signature_key: self.signer.public().into(),
        }
    }

    /// Key packages others add this device with, each good for one add.
    pub fn key_packages(&self, count: usize) -> Result<Vec<Vec<u8>>> {
        (0..count).map(|_| self.key_package(false)).collect()
    }

    /// The key package the server hands out when the others ran out, used
    /// again and again until it's replaced.
    pub fn last_resort_key_package(&self) -> Result<Vec<u8>> {
        self.key_package(true)
    }

    fn key_package(&self, last_resort: bool) -> Result<Vec<u8>> {
        let mut builder = KeyPackage::builder();
        if last_resort {
            builder = builder
                .mark_as_last_resort()
                .leaf_node_capabilities(Capabilities::builder().extensions(vec![ExtensionType::LastResort]).build());
        }
        let bundle = builder.build(SUITE, &self.provider, &self.signer, self.credential()).map_err(mls)?;
        MlsMessageOut::from(bundle.key_package().clone()).to_bytes().map_err(mls)
    }

    fn group_id(conversation_id: &str) -> GroupId {
        GroupId::from_slice(conversation_id.as_bytes())
    }

    fn load(&self, conversation_id: &str) -> Result<MlsGroup> {
        MlsGroup::load(self.provider.storage(), &Self::group_id(conversation_id))
            .map_err(mls)?
            .filter(MlsGroup::is_active)
            .ok_or(Error::NotMember)
    }

    /// Whether this device is in the conversation's group.
    pub fn is_member(&self, conversation_id: &str) -> bool {
        self.load(conversation_id).is_ok()
    }

    /// The epoch the device's copy of the group is at.
    pub fn epoch(&self, conversation_id: &str) -> Result<u64> {
        Ok(self.load(conversation_id)?.epoch().as_u64())
    }

    /// A secret every device in the conversation's group gets alike at its
    /// current epoch, and nobody else (MLS's exporter, RFC 9420 section 8.5),
    /// with that epoch. Calls in direct messages encrypt their sound with it.
    pub fn export_secret(&self, conversation_id: &str, label: &str, length: usize) -> Result<(u64, Vec<u8>)> {
        let group = self.load(conversation_id)?;
        let secret =
            group.export_secret(self.provider.crypto(), label, conversation_id.as_bytes(), length).map_err(mls)?;
        Ok((group.epoch().as_u64(), secret))
    }

    /// Whether the device has a commit of its own waiting on the server.
    pub fn has_pending_commit(&self, conversation_id: &str) -> Result<bool> {
        Ok(self.load(conversation_id)?.pending_commit().is_some())
    }

    /// Every device in the conversation, this one included.
    pub fn members(&self, conversation_id: &str) -> Result<Vec<Member>> {
        members(&self.load(conversation_id)?)
    }

    /// Starts the conversation's group with this device alone in it, at
    /// epoch 0. The server only learns of it from the first commit.
    pub fn create_group(&self, conversation_id: &str) -> Result<()> {
        self.forget(conversation_id)?;
        MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &create_config(),
            Self::group_id(conversation_id),
            self.credential(),
        )
        .map_err(mls)?;
        Ok(())
    }

    /// Drops the device's copy of a conversation's group, such as after it
    /// was removed, or when its first commit lost to someone else's.
    pub fn forget(&self, conversation_id: &str) -> Result<()> {
        if let Some(mut group) =
            MlsGroup::load(self.provider.storage(), &Self::group_id(conversation_id)).map_err(mls)?
        {
            group.delete(self.provider.storage()).map_err(mls)?;
        }
        Ok(())
    }

    /// A commit adding devices (one key package each, as `(device id, key
    /// package)`) and removing others (by device id). It waits as this
    /// device's pending commit until the server takes it ([`Device::process`]
    /// of it merges it) or turns it down ([`Device::discard_pending`]).
    pub fn commit(
        &self,
        conversation_id: &str,
        adds: &[(String, Vec<u8>)],
        removes: &[String],
        allowed: &[String],
    ) -> Result<Commit> {
        let mut group = self.load(conversation_id)?;
        if group.pending_commit().is_some() {
            group.clear_pending_commit(self.provider.storage()).map_err(mls)?;
        }
        let current = members(&group)?;
        let mut key_packages = Vec::with_capacity(adds.len());
        let mut added = Vec::with_capacity(adds.len());
        for (expected, bytes) in adds {
            let key_package = self.validate_key_package(bytes)?;
            let member = member_of(key_package.leaf_node())?;
            if &member.device_id != expected {
                return Err(Error::Invalid(format!("that key package isn't device {expected}'s")));
            }
            check_allowed(&member, allowed)?;
            if current.iter().any(|m| m.device_id == member.device_id) || added.contains(&member) {
                continue;
            }
            key_packages.push(key_package);
            added.push(member);
        }
        let own = self.device_id();
        let removed: Vec<Member> =
            current.into_iter().filter(|m| m.device_id != own && removes.contains(&m.device_id)).collect();
        let leaves: Vec<LeafNodeIndex> = group
            .members()
            .filter(|m| removed.iter().any(|r| r.signature_key == m.signature_key))
            .map(|m| m.index)
            .collect();
        let bundle = group
            .commit_builder()
            .propose_adds(key_packages)
            .propose_removals(leaves)
            .load_psks(self.provider.storage())
            .map_err(mls)?
            .build(self.provider.rand(), self.provider.crypto(), &self.signer, |_| true)
            .map_err(mls)?
            .stage_commit(&self.provider)
            .map_err(mls)?;
        let (commit, welcome, group_info) = bundle.into_messages();
        let group_info = group_info.ok_or_else(|| Error::Mls("the commit came without a group info".into()))?;
        Ok(Commit {
            commit: commit.to_bytes().map_err(mls)?,
            group_info: group_info.to_bytes().map_err(mls)?,
            welcome: welcome.map(|w| w.to_bytes()).transpose().map_err(mls)?,
            added,
            removed,
        })
    }

    /// Throws away a commit the server turned down.
    pub fn discard_pending(&self, conversation_id: &str) -> Result<()> {
        let mut group = self.load(conversation_id)?;
        group.clear_pending_commit(self.provider.storage()).map_err(mls)
    }

    /// Joins a conversation another device added this one to. Returns the
    /// epoch it joined at: records from before it stay unreadable here.
    pub fn join_from_welcome(&self, conversation_id: &str, welcome: &[u8], allowed: &[String]) -> Result<u64> {
        let MlsMessageBodyIn::Welcome(welcome) =
            MlsMessageIn::tls_deserialize_exact(welcome).map_err(invalid)?.extract()
        else {
            return Err(invalid("that isn't a welcome"));
        };
        let staged = StagedWelcome::new_from_welcome(&self.provider, &join_config(), welcome, None).map_err(mls)?;
        if staged.group_context().group_id() != &Self::group_id(conversation_id) {
            return Err(Error::Invalid("that welcome is for another conversation".into()));
        }
        for member in staged.members() {
            check_allowed(&member_from(&member.credential, &member.signature_key)?, allowed)?;
        }
        self.forget(conversation_id)?;
        let group = staged.into_group(&self.provider).map_err(mls)?;
        Ok(group.epoch().as_u64())
    }

    /// Joins a conversation by itself, from the group info the server keeps:
    /// for a new device nobody has added yet. Unlike other commits, this one
    /// takes effect at once: if the server turns it down, [`Device::forget`]
    /// the conversation and try again.
    pub fn join_by_itself(&self, conversation_id: &str, group_info: &[u8], allowed: &[String]) -> Result<Commit> {
        let MlsMessageBodyIn::GroupInfo(info) =
            MlsMessageIn::tls_deserialize_exact(group_info).map_err(invalid)?.extract()
        else {
            return Err(invalid("that isn't a group info"));
        };
        if info.group_id() != &Self::group_id(conversation_id) {
            return Err(Error::Invalid("that group info is for another conversation".into()));
        }
        self.forget(conversation_id)?;
        let (mut group, bundle) = MlsGroup::external_commit_builder()
            .with_config(join_config())
            .build_group(&self.provider, info, self.credential())
            .map_err(mls)?
            .load_psks(self.provider.storage())
            .map_err(mls)?
            .build(self.provider.rand(), self.provider.crypto(), &self.signer, |_| true)
            .map_err(mls)?
            .finalize(&self.provider)
            .map_err(mls)?;
        // Only join a group that holds nobody else.
        let checked = members(&group).and_then(|all| all.iter().try_for_each(|member| check_allowed(member, allowed)));
        if let Err(err) = checked {
            let _ = group.delete(self.provider.storage());
            return Err(err);
        }
        let (commit, _, group_info) = bundle.into_messages();
        let group_info = group_info.ok_or_else(|| Error::Mls("the commit came without a group info".into()))?;
        let me = Member {
            user_id: self.user_id.clone(),
            device_id: self.device_id(),
            signature_key: self.signer.public().to_vec(),
        };
        Ok(Commit {
            commit: commit.to_bytes().map_err(mls)?,
            group_info: group_info.to_bytes().map_err(mls)?,
            welcome: None,
            added: vec![me],
            removed: vec![],
        })
    }

    /// Seals a message for everyone in the conversation.
    pub fn encrypt(&self, conversation_id: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut group = self.load(conversation_id)?;
        let message = group.create_message(&self.provider, &self.signer, plaintext).map_err(mls)?;
        message.to_bytes().map_err(mls)
    }

    /// Opens the next record of a conversation, in the order the server
    /// keeps them. `own` says the server has it from this device.
    pub fn process(&self, conversation_id: &str, bytes: &[u8], own: bool, allowed: &[String]) -> Result<Processed> {
        let mut group = self.load(conversation_id)?;
        let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(invalid)?;
        let message = message.try_into_protocol_message().map_err(invalid)?;
        if message.group_id() != group.group_id() {
            return Err(Error::Invalid("that message is for another conversation".into()));
        }
        let epoch = message.epoch().as_u64();
        let current = group.epoch().as_u64();
        if epoch < current {
            return Ok(Processed::Stale);
        }
        if epoch > current {
            return Err(Error::Behind);
        }
        let before = members(&group)?;
        if own {
            if message.content_type() != ContentType::Commit {
                return Ok(Processed::Own);
            }
            if group.pending_commit().is_none() {
                return Ok(Processed::Stale);
            }
            group.merge_pending_commit(&self.provider).map_err(mls)?;
            let after = members(&group)?;
            let (added, removed) = difference(&before, &after);
            let by = before.iter().find(|m| m.device_id == self.device_id()).cloned();
            return Ok(Processed::Commit { by, added, removed, removed_me: false });
        }
        // Someone else's commit beat this device's own to the server.
        if message.content_type() == ContentType::Commit && group.pending_commit().is_some() {
            group.clear_pending_commit(self.provider.storage()).map_err(mls)?;
        }
        let processed = group.process_message(&self.provider, message).map_err(mls)?;
        let sender = match processed.sender() {
            Sender::Member(index) => group
                .members()
                .find(|m| m.index == *index)
                .map(|m| member_from(&m.credential, &m.signature_key))
                .transpose()?,
            _ => None,
        };
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(message) => {
                let sender = sender.ok_or_else(|| Error::Invalid("a message from outside the group".into()))?;
                Ok(Processed::Message { sender, plaintext: message.into_bytes() })
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                for proposal in staged.queued_proposals() {
                    if !matches!(
                        proposal.proposal(),
                        Proposal::Add(_) | Proposal::Remove(_) | Proposal::Update(_) | Proposal::ExternalInit(_)
                    ) {
                        return Err(Error::Invalid("that commit changes more than who's in the conversation".into()));
                    }
                }
                for credential in staged.credentials_to_verify() {
                    check_identity(credential, allowed)?;
                }
                let joiner = match (&sender, staged.update_path_leaf_node()) {
                    (None, Some(leaf)) => Some(member_of(leaf)?),
                    _ => None,
                };
                let removed_me = staged.self_removed();
                group.merge_staged_commit(&self.provider, *staged).map_err(mls)?;
                if removed_me {
                    let (_, removed) = difference(&before, &[]);
                    let _ = group.delete(self.provider.storage());
                    return Ok(Processed::Commit { by: sender.or(joiner), added: vec![], removed, removed_me });
                }
                let after = members(&group)?;
                let (added, removed) = difference(&before, &after);
                Ok(Processed::Commit { by: sender.or(joiner), added, removed, removed_me })
            }
            ProcessedMessageContent::OwnPendingCommit => {
                group.merge_pending_commit(&self.provider).map_err(mls)?;
                let after = members(&group)?;
                let (added, removed) = difference(&before, &after);
                let by = before.iter().find(|m| m.device_id == self.device_id()).cloned();
                Ok(Processed::Commit { by, added, removed, removed_me: false })
            }
            ProcessedMessageContent::OwnPrivateMessage => Ok(Processed::Own),
            // Proposals on their own aren't used here; only commits change a conversation.
            ProcessedMessageContent::ProposalMessage(_) | ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                Ok(Processed::Stale)
            }
        }
    }

    fn validate_key_package(&self, bytes: &[u8]) -> Result<KeyPackage> {
        let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(invalid)?;
        let MlsMessageBodyIn::KeyPackage(key_package) = message.extract() else {
            return Err(invalid("that isn't a key package"));
        };
        let key_package = key_package.validate(self.provider.crypto(), ProtocolVersion::Mls10).map_err(invalid)?;
        if key_package.ciphersuite() != SUITE {
            return Err(invalid("that key package is for another cipher suite"));
        }
        Ok(key_package)
    }
}

fn create_config() -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder()
        .ciphersuite(SUITE)
        .use_ratchet_tree_extension(true)
        .wire_format_policy(MIXED_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .padding_size(PADDING)
        .sender_ratchet_configuration(SenderRatchetConfiguration::new(OUT_OF_ORDER, MAX_FORWARD))
        .build()
}

fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .wire_format_policy(MIXED_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .padding_size(PADDING)
        .sender_ratchet_configuration(SenderRatchetConfiguration::new(OUT_OF_ORDER, MAX_FORWARD))
        .build()
}

fn members(group: &MlsGroup) -> Result<Vec<Member>> {
    let mut all: Vec<Member> =
        group.members().map(|m| member_from(&m.credential, &m.signature_key)).collect::<Result<_>>()?;
    all.sort();
    Ok(all)
}

fn member_of(leaf: &LeafNode) -> Result<Member> {
    member_from(leaf.credential(), leaf.signature_key().as_slice())
}

fn member_from(credential: &Credential, signature_key: &[u8]) -> Result<Member> {
    let basic = BasicCredential::try_from(credential.clone()).map_err(invalid)?;
    let user_id = String::from_utf8(basic.identity().to_vec()).map_err(invalid)?;
    Ok(Member { user_id, device_id: device_id(signature_key), signature_key: signature_key.to_vec() })
}

fn check_identity(credential: &Credential, allowed: &[String]) -> Result<()> {
    let basic = BasicCredential::try_from(credential.clone()).map_err(invalid)?;
    if allowed.iter().any(|user| user.as_bytes() == basic.identity()) {
        Ok(())
    } else {
        Err(Error::Intruder("someone who isn't in this conversation would have been let in".into()))
    }
}

fn check_allowed(member: &Member, allowed: &[String]) -> Result<()> {
    if allowed.contains(&member.user_id) {
        Ok(())
    } else {
        Err(Error::Intruder(format!("{} isn't in this conversation", member.user_id)))
    }
}

/// Who's in `after` but not `before`, and the other way round.
fn difference(before: &[Member], after: &[Member]) -> (Vec<Member>, Vec<Member>) {
    let added = after.iter().filter(|m| !before.contains(m)).cloned().collect();
    let removed = before.iter().filter(|m| !after.contains(m)).cloned().collect();
    (added, removed)
}
