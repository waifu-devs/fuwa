//! Reading what the server needs from MLS messages (RFC 9420, section 6)
//! without decrypting anything: which conversation and epoch a message is
//! for, what kind it is, and the public keys in a key package. Everything
//! else in them is either encrypted or the clients' business.

use std::fmt;

/// MLS 1.0, the only version there is.
const MLS10: u16 = 1;

/// The extension marking a key package as last resort (from the MLS
/// extensions draft, as OpenMLS numbers it).
const LAST_RESORT: u16 = 10;

/// How an MLS message is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    /// Signed but not encrypted: only devices joining by themselves (external
    /// commits) send these here.
    PublicMessage,
    /// Encrypted for the group: messages, and commits from members.
    PrivateMessage,
    /// Brings devices added by a commit into the group.
    Welcome,
    /// The group's public state, for joining it by yourself.
    GroupInfo,
    /// What a device publishes so others can add it.
    KeyPackage,
}

/// What a public or private message carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Application,
    Proposal,
    /// Changes who's in the group, and moves it to the next epoch.
    Commit,
}

/// Who sent a public message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    Member(u32),
    External(u32),
    NewMemberProposal,
    /// A device joining by itself.
    NewMemberCommit,
}

/// The unencrypted header of a public or private message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHeader<'a> {
    pub wire_format: WireFormat,
    pub group_id: &'a [u8],
    pub epoch: u64,
    pub content_type: ContentType,
    /// Only public messages say who sent them; private ones encrypt it.
    pub sender: Option<Sender>,
}

/// The start of a group info: the group and the epoch it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfoHeader<'a> {
    pub cipher_suite: u16,
    pub group_id: &'a [u8],
    pub epoch: u64,
}

/// The public parts of a key package that say whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPackageInfo<'a> {
    pub cipher_suite: u16,
    /// The device's signing key, which names the device (`crate::device_id`).
    pub signature_key: &'a [u8],
    /// The basic credential's identity: the account id. Empty for other kinds of credential.
    pub identity: &'a [u8],
    /// When it stops being valid, in seconds since 1970.
    pub not_after: u64,
    /// Kept for whenever the device has no others left, rather than used once.
    pub last_resort: bool,
}

/// Bytes that aren't the MLS message they claim to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a valid MLS message: {}", self.0)
    }
}

impl std::error::Error for Malformed {}

type Result<T> = std::result::Result<T, Malformed>;

/// The header of an MLS message carrying a public or private message.
pub fn message_header(bytes: &[u8]) -> Result<MessageHeader<'_>> {
    let mut r = Reader(bytes);
    let wire_format = r.envelope()?;
    match wire_format {
        WireFormat::PublicMessage => {
            let group_id = r.vector()?;
            let epoch = r.u64()?;
            let sender = match r.u8()? {
                1 => Sender::Member(r.u32()?),
                2 => Sender::External(r.u32()?),
                3 => Sender::NewMemberProposal,
                4 => Sender::NewMemberCommit,
                _ => return Err(Malformed("unknown sender type")),
            };
            r.vector()?; // authenticated data
            let content_type = r.content_type()?;
            Ok(MessageHeader { wire_format, group_id, epoch, content_type, sender: Some(sender) })
        }
        WireFormat::PrivateMessage => {
            let group_id = r.vector()?;
            let epoch = r.u64()?;
            let content_type = r.content_type()?;
            Ok(MessageHeader { wire_format, group_id, epoch, content_type, sender: None })
        }
        _ => Err(Malformed("expected a public or private message")),
    }
}

/// The header of an MLS message carrying a group info.
pub fn group_info_header(bytes: &[u8]) -> Result<GroupInfoHeader<'_>> {
    let mut r = Reader(bytes);
    if r.envelope()? != WireFormat::GroupInfo {
        return Err(Malformed("expected a group info"));
    }
    // GroupContext: version, cipher suite, group id, epoch, ...
    if r.u16()? != MLS10 {
        return Err(Malformed("unknown protocol version"));
    }
    let cipher_suite = r.u16()?;
    let group_id = r.vector()?;
    let epoch = r.u64()?;
    Ok(GroupInfoHeader { cipher_suite, group_id, epoch })
}

/// Whether an MLS message carries a welcome.
pub fn is_welcome(bytes: &[u8]) -> bool {
    Reader(bytes).envelope() == Ok(WireFormat::Welcome)
}

/// The public parts of an MLS message carrying a key package.
pub fn key_package_info(bytes: &[u8]) -> Result<KeyPackageInfo<'_>> {
    let mut r = Reader(bytes);
    if r.envelope()? != WireFormat::KeyPackage {
        return Err(Malformed("expected a key package"));
    }
    if r.u16()? != MLS10 {
        return Err(Malformed("unknown protocol version"));
    }
    let cipher_suite = r.u16()?;
    r.vector()?; // init key
    // LeafNode: encryption key, signature key, credential, capabilities, source.
    r.vector()?;
    let signature_key = r.vector()?;
    let identity = match r.u16()? {
        1 => r.vector()?, // basic
        2 => {
            r.vector()?; // x509 certificates
            &[]
        }
        _ => return Err(Malformed("unknown credential type")),
    };
    for _ in 0..5 {
        r.vector()?; // versions, cipher suites, extensions, proposals, credentials
    }
    if r.u8()? != 1 {
        return Err(Malformed("a key package's leaf must come from the key package"));
    }
    r.u64()?; // not before
    let not_after = r.u64()?;
    extension_types(r.vector()?)?; // the leaf's
    r.vector()?; // the leaf's signature
    let extensions = extension_types(r.vector()?)?;
    r.vector()?; // the key package's signature
    if !r.0.is_empty() {
        return Err(Malformed("trailing bytes"));
    }
    let last_resort = extensions.contains(&LAST_RESORT);
    Ok(KeyPackageInfo { cipher_suite, signature_key, identity, not_after, last_resort })
}

/// The types of the extensions in an encoded list of them.
fn extension_types(bytes: &[u8]) -> Result<Vec<u16>> {
    let mut r = Reader(bytes);
    let mut types = Vec::new();
    while !r.0.is_empty() {
        types.push(r.u16()?);
        r.vector()?;
    }
    Ok(types)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Malformed("too short"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().expect("two bytes")))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    /// A variable-length vector, `<V>` in the RFC: a length (1, 2 or 4 bytes,
    /// the top two bits saying which, always as short as it can be), then
    /// that many bytes.
    fn vector(&mut self) -> Result<&'a [u8]> {
        let first = self.u8()?;
        let length = match first >> 6 {
            0 => u32::from(first),
            1 => {
                let length = (u32::from(first & 0x3f) << 8) | u32::from(self.u8()?);
                if length < 1 << 6 {
                    return Err(Malformed("length not minimally encoded"));
                }
                length
            }
            2 => {
                let rest = self.take(3)?;
                let length = (u32::from(first & 0x3f) << 24)
                    | (u32::from(rest[0]) << 16)
                    | (u32::from(rest[1]) << 8)
                    | u32::from(rest[2]);
                if length < 1 << 14 {
                    return Err(Malformed("length not minimally encoded"));
                }
                length
            }
            _ => return Err(Malformed("invalid length")),
        };
        self.take(length as usize)
    }

    /// The version and wire format every MLS message starts with.
    fn envelope(&mut self) -> Result<WireFormat> {
        if self.u16()? != MLS10 {
            return Err(Malformed("unknown protocol version"));
        }
        Ok(match self.u16()? {
            1 => WireFormat::PublicMessage,
            2 => WireFormat::PrivateMessage,
            3 => WireFormat::Welcome,
            4 => WireFormat::GroupInfo,
            5 => WireFormat::KeyPackage,
            _ => return Err(Malformed("unknown wire format")),
        })
    }

    fn content_type(&mut self) -> Result<ContentType> {
        match self.u8()? {
            1 => Ok(ContentType::Application),
            2 => Ok(ContentType::Proposal),
            3 => Ok(ContentType::Commit),
            _ => Err(Malformed("unknown content type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_private_message_header() {
        // version 1, private message, group id "ab", epoch 7, commit, then whatever.
        let bytes = [0, 1, 0, 2, 2, b'a', b'b', 0, 0, 0, 0, 0, 0, 0, 7, 3, 0, 0, 0];
        let header = message_header(&bytes).unwrap();
        assert_eq!(header.wire_format, WireFormat::PrivateMessage);
        assert_eq!(header.group_id, b"ab");
        assert_eq!(header.epoch, 7);
        assert_eq!(header.content_type, ContentType::Commit);
        assert_eq!(header.sender, None);
    }

    #[test]
    fn reads_a_public_message_header() {
        // version 1, public message, group id "g", epoch 2, new member commit, no aad, commit.
        let bytes = [0, 1, 0, 1, 1, b'g', 0, 0, 0, 0, 0, 0, 0, 2, 4, 0, 3, 9, 9];
        let header = message_header(&bytes).unwrap();
        assert_eq!(header.wire_format, WireFormat::PublicMessage);
        assert_eq!(header.epoch, 2);
        assert_eq!(header.sender, Some(Sender::NewMemberCommit));
        assert_eq!(header.content_type, ContentType::Commit);
    }

    #[test]
    fn refuses_what_isnt_mls() {
        assert!(message_header(b"").is_err());
        assert!(message_header(b"hello there").is_err());
        assert!(message_header(&[0, 2, 0, 2, 0]).is_err()); // version 2
        assert!(message_header(&[0, 1, 0, 3, 0]).is_err()); // a welcome
        assert!(message_header(&[0, 1, 0, 2, 0x40, 0x01]).is_err()); // a 2-byte length for 1
        assert!(message_header(&[0, 1, 0, 2, 0xc0]).is_err()); // a reserved length prefix
        assert!(message_header(&[0, 1, 0, 2, 5, b'a']).is_err()); // shorter than it says
    }

    #[test]
    fn reads_long_lengths() {
        let mut bytes = vec![0, 1, 0, 2, 0x40, 0x80];
        bytes.extend([7u8; 128]);
        bytes.extend([0, 0, 0, 0, 0, 0, 1, 0, 1]);
        let header = message_header(&bytes).unwrap();
        assert_eq!(header.group_id.len(), 128);
        assert_eq!(header.epoch, 256);
        assert_eq!(header.content_type, ContentType::Application);
    }
}
