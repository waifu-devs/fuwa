//! SAML 2.0, as a service provider: an AuthnRequest sent with the
//! HTTP-Redirect binding, and the provider's Response taken with HTTP-POST.
//! A response is believed only when its assertion is signed (itself, or the
//! whole response) by one of the provider's certificates, the reference
//! points at exactly that element by an ID no other element has, and it was
//! made for this request, for this side and for now.

use std::io::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use roxmltree::Node;
use rustls_pki_types::CertificateDer;
use sha2::{Digest, Sha256, Sha384, Sha512};

use super::xml::{self, children, exc_c14n, only_child, text_of};
use super::{Identity, Provider};
use crate::error::{Error, Result};

pub const PROTOCOL: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
pub const ASSERTION: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
const DSIG: &str = "http://www.w3.org/2000/09/xmldsig#";
const EXC_C14N: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
const EXC_C14N_COMMENTS: &str = "http://www.w3.org/2001/10/xml-exc-c14n#WithComments";
const ENVELOPED: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
const SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";
const BEARER: &str = "urn:oasis:names:tc:SAML:2.0:cm:bearer";
const POST_BINDING: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST";
const EMAIL_FORMAT: &str = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress";

/// How far apart this side's clock and the provider's may be.
const SKEW_MS: i64 = 3 * 60 * 1000;

/// The address of the provider's sign-in page for one request, and the
/// request's ID (which the response must answer).
pub fn authorize_url(
    provider: &Provider,
    entity_id: &str,
    acs_url: &str,
    relay_state: &str,
    now: i64,
) -> Result<(String, String)> {
    let id = format!("_{}", crate::auth::new_token());
    let request = format!(
        r#"<samlp:AuthnRequest xmlns:samlp="{PROTOCOL}" xmlns:saml="{ASSERTION}" ID="{id}" Version="2.0" IssueInstant="{instant}" Destination="{destination}" AssertionConsumerServiceURL="{acs}" ProtocolBinding="{POST_BINDING}"><saml:Issuer>{issuer}</saml:Issuer><samlp:NameIDPolicy AllowCreate="true"/></samlp:AuthnRequest>"#,
        instant = xml::format_time(now),
        destination = xml::escape(&provider.saml_sso_url),
        acs = xml::escape(acs_url),
        issuer = xml::escape(entity_id),
    );
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    deflate.write_all(request.as_bytes()).map_err(|err| Error::internal(err.to_string()))?;
    let deflated = deflate.finish().map_err(|err| Error::internal(err.to_string()))?;
    let mut url = reqwest::Url::parse(&provider.saml_sso_url)
        .map_err(|_| Error::FailedPrecondition("the SAML sign-in URL isn't a URL".into()))?;
    url.query_pairs_mut().append_pair("SAMLRequest", &STANDARD.encode(deflated)).append_pair("RelayState", relay_state);
    Ok((url.into(), id))
}

/// The certificates in `text`: PEM blocks, or bare base64 as metadata has them.
pub fn certificates(text: &str) -> Result<Vec<Vec<u8>>> {
    let invalid = || Error::invalid("the SAML certificates must be PEM (-----BEGIN CERTIFICATE-----)");
    let mut found = Vec::new();
    let blocks: Vec<String> = if text.contains("-----BEGIN") {
        text.split("-----BEGIN CERTIFICATE-----")
            .skip(1)
            .map(|block| block.split("-----END CERTIFICATE-----").next().unwrap_or_default().to_string())
            .collect()
    } else {
        vec![text.to_string()]
    };
    for block in blocks {
        let compact: String = block.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.is_empty() {
            continue;
        }
        let der = STANDARD.decode(compact).map_err(|_| invalid())?;
        webpki::EndEntityCert::try_from(&CertificateDer::from(der.as_slice()))
            .map_err(|err| Error::invalid(format!("a SAML certificate couldn't be read: {err:?}")))?;
        found.push(der);
    }
    if found.is_empty() || found.len() > 4 {
        return Err(Error::invalid("give one to four SAML signing certificates"));
    }
    Ok(found)
}

/// What a response must match.
pub struct Expect<'a> {
    pub provider: &'a Provider,
    /// This side's entity ID: the audience.
    pub entity_id: &'a str,
    pub acs_url: &'a str,
    /// The AuthnRequest's ID.
    pub request_id: &'a str,
    pub now: i64,
}

/// Reads and checks a SAMLResponse as it was posted (base64).
pub fn identify(posted: &str, expect: &Expect) -> Result<Identity> {
    let refused = |why: &str| Error::denied(format!("the identity provider's answer was refused: {why}"));
    let compact: String = posted.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD.decode(compact).map_err(|_| refused("it isn't base64"))?;
    let text = String::from_utf8(bytes).map_err(|_| refused("it isn't UTF-8"))?;
    let doc = xml::parse(&text)?;
    let response = doc.root_element();
    if response.tag_name().namespace() != Some(PROTOCOL) || response.tag_name().name() != "Response" {
        return Err(refused("it isn't a SAML Response"));
    }

    // Signature wrapping hides a second element under a signed one's ID, so
    // every ID must be unique, and the assertion read is the one checked.
    let mut ids = std::collections::HashSet::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        for attribute in node.attributes().filter(|a| a.name() == "ID" || a.name() == "Id" || a.name() == "id") {
            if !ids.insert(attribute.value()) {
                return Err(refused("two elements share an ID"));
            }
        }
    }

    let status = only_child(response, PROTOCOL, "Status")
        .and_then(|s| only_child(s, PROTOCOL, "StatusCode"))
        .and_then(|c| c.attribute("Value"));
    if status != Some(SUCCESS) {
        let message = only_child(response, PROTOCOL, "Status")
            .and_then(|s| only_child(s, PROTOCOL, "StatusMessage"))
            .map(text_of)
            .unwrap_or_default();
        return Err(Error::denied(if message.is_empty() {
            "the identity provider didn't sign you in".to_string()
        } else {
            format!("the identity provider didn't sign you in: {message}")
        }));
    }
    if children(response, ASSERTION, "EncryptedAssertion").next().is_some() {
        return Err(refused("encrypted assertions aren't supported; turn assertion encryption off for fuwa"));
    }
    let assertion =
        only_child(response, ASSERTION, "Assertion").ok_or_else(|| refused("it needs exactly one assertion"))?;

    let certificates = certificates(&expect.provider.saml_certificates)?;
    let signed = [assertion, response].into_iter().any(|element| match only_child(element, DSIG, "Signature") {
        Some(signature) => verify(&text, element, signature, &certificates).is_ok(),
        None => false,
    });
    if !signed {
        let why = [assertion, response]
            .into_iter()
            .filter_map(|element| {
                only_child(element, DSIG, "Signature").map(|s| verify(&text, element, s, &certificates))
            })
            .find_map(|result| result.err());
        return Err(match why {
            Some(err) => refused(&err),
            None => refused("its assertion isn't signed"),
        });
    }

    if let Some(destination) = response.attribute("Destination")
        && destination != expect.acs_url
    {
        return Err(refused("it was sent to another address"));
    }
    if let Some(in_response_to) = response.attribute("InResponseTo")
        && in_response_to != expect.request_id
    {
        return Err(refused("it answers another sign-in"));
    }
    for issuer in
        [only_child(response, ASSERTION, "Issuer"), only_child(assertion, ASSERTION, "Issuer")].into_iter().flatten()
    {
        if text_of(issuer) != expect.provider.saml_entity_id {
            return Err(refused("it comes from another identity provider"));
        }
    }
    if only_child(assertion, ASSERTION, "Issuer").is_none() {
        return Err(refused("its assertion names no issuer"));
    }

    let conditions = only_child(assertion, ASSERTION, "Conditions").ok_or_else(|| refused("it has no conditions"))?;
    within(conditions, expect.now).map_err(refused)?;
    let audiences: Vec<String> = children(conditions, ASSERTION, "AudienceRestriction")
        .flat_map(|r| children(r, ASSERTION, "Audience").map(text_of))
        .collect();
    if !audiences.iter().any(|a| a == expect.entity_id) {
        return Err(refused(&format!(
            "it's meant for another service (give the provider the entity ID {})",
            expect.entity_id
        )));
    }

    let subject = only_child(assertion, ASSERTION, "Subject").ok_or_else(|| refused("it names nobody"))?;
    let name_id = only_child(subject, ASSERTION, "NameID").ok_or_else(|| refused("it names nobody"))?;
    let confirmed = children(subject, ASSERTION, "SubjectConfirmation").any(|confirmation| {
        confirmation.attribute("Method") == Some(BEARER)
            && only_child(confirmation, ASSERTION, "SubjectConfirmationData").is_some_and(|data| {
                data.attribute("Recipient") == Some(expect.acs_url)
                    && data.attribute("InResponseTo") == Some(expect.request_id)
                    && data
                        .attribute("NotOnOrAfter")
                        .and_then(xml::parse_time)
                        .is_some_and(|t| expect.now < t + SKEW_MS)
            })
    });
    if !confirmed {
        return Err(refused("it isn't confirmed for this sign-in, this address and now"));
    }

    // A transient NameID is new on every sign-in, so it can't say who
    // someone is from one time to the next.
    if name_id.attribute("Format") == Some("urn:oasis:names:tc:SAML:2.0:nameid-format:transient") {
        return Err(refused("its NameID is transient; set the provider to send a persistent one or an email"));
    }
    let subject_id = text_of(name_id);
    if subject_id.is_empty() || subject_id.len() > 255 {
        return Err(refused("its NameID is empty or too long"));
    }
    let attributes = attributes(assertion);
    let pick = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| attributes.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()))
    };
    let email = pick(&[
        "email",
        "mail",
        "emailaddress",
        "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress",
        "urn:oid:0.9.2342.19200300.100.1.3",
    ])
    .or_else(|| (name_id.attribute("Format") == Some(EMAIL_FORMAT)).then(|| subject_id.clone()))
    .unwrap_or_default();
    let name = pick(&[
        "displayName",
        "name",
        "http://schemas.microsoft.com/identity/claims/displayname",
        "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name",
        "urn:oid:2.16.840.1.113730.3.1.241",
    ])
    .or_else(|| {
        let given = pick(&[
            "givenName",
            "firstName",
            "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/givenname",
            "urn:oid:2.5.4.42",
        ]);
        let family = pick(&[
            "sn",
            "surname",
            "lastName",
            "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/surname",
            "urn:oid:2.5.4.4",
        ]);
        let full = [given, family].into_iter().flatten().collect::<Vec<_>>().join(" ");
        (!full.is_empty()).then_some(full)
    })
    .unwrap_or_default();
    let username =
        pick(&["username", "uid", "preferred_username", "urn:oid:0.9.2342.19200300.100.1.1"]).unwrap_or_default();
    Ok(Identity {
        subject: subject_id,
        email: email.to_lowercase(),
        email_verified: true,
        name,
        username,
        picture: None,
    })
}

/// Whether now falls inside the assertion's Conditions.
fn within(conditions: Node, now: i64) -> std::result::Result<(), &'static str> {
    if let Some(not_before) = conditions.attribute("NotBefore") {
        let t = xml::parse_time(not_before).ok_or("its NotBefore isn't a time")?;
        if now + SKEW_MS < t {
            return Err("it isn't valid yet (is a clock wrong?)");
        }
    }
    if let Some(not_on_or_after) = conditions.attribute("NotOnOrAfter") {
        let t = xml::parse_time(not_on_or_after).ok_or("its NotOnOrAfter isn't a time")?;
        if now >= t + SKEW_MS {
            return Err("it ran out");
        }
    }
    Ok(())
}

/// The assertion's attributes as (name, first value).
fn attributes(assertion: Node) -> Vec<(String, String)> {
    children(assertion, ASSERTION, "AttributeStatement")
        .flat_map(|statement| children(statement, ASSERTION, "Attribute"))
        .filter_map(|attribute| {
            let value = children(attribute, ASSERTION, "AttributeValue").map(text_of).find(|v| !v.is_empty())?;
            let names = [attribute.attribute("Name"), attribute.attribute("FriendlyName")];
            Some(names.into_iter().flatten().map(move |name| (name.to_string(), value.clone())).collect::<Vec<_>>())
        })
        .flatten()
        .collect()
}

/// Checks `signature` signs exactly `element`, with one of `certificates`.
fn verify(text: &str, element: Node, signature: Node, certificates: &[Vec<u8>]) -> std::result::Result<(), String> {
    let id = element.attribute("ID").filter(|id| !id.is_empty()).ok_or("the signed element has no ID")?;
    let signed_info = only_child(signature, DSIG, "SignedInfo").ok_or("its signature has no SignedInfo")?;
    let c14n_method = only_child(signed_info, DSIG, "CanonicalizationMethod").and_then(|m| m.attribute("Algorithm"));
    let with_comments = match c14n_method {
        Some(EXC_C14N) => false,
        Some(EXC_C14N_COMMENTS) => true,
        _ => return Err("its signature must use exclusive canonicalization".into()),
    };
    let method =
        only_child(signed_info, DSIG, "SignatureMethod").and_then(|m| m.attribute("Algorithm")).unwrap_or_default();
    let mut references = children(signed_info, DSIG, "Reference");
    let reference = references.next().ok_or("its signature references nothing")?;
    if references.next().is_some() {
        return Err("its signature must reference exactly one element".into());
    }
    if reference.attribute("URI") != Some(format!("#{id}").as_str()) {
        return Err("its signature references another element".into());
    }

    let mut inclusive = Vec::new();
    let mut canonical = false;
    if let Some(transforms) = only_child(reference, DSIG, "Transforms") {
        for transform in children(transforms, DSIG, "Transform") {
            match transform.attribute("Algorithm") {
                Some(ENVELOPED) => {}
                Some(EXC_C14N) => {
                    canonical = true;
                    if let Some(list) = transform
                        .children()
                        .find(|c| c.is_element() && c.tag_name().name() == "InclusiveNamespaces")
                        .and_then(|c| c.attribute("PrefixList"))
                    {
                        inclusive = list.split_whitespace().map(str::to_string).collect();
                    }
                }
                _ => return Err("its signature uses a transform fuwa doesn't take".into()),
            }
        }
    }
    if !canonical {
        return Err("its signature must use exclusive canonicalization".into());
    }

    let digest_method =
        only_child(reference, DSIG, "DigestMethod").and_then(|m| m.attribute("Algorithm")).unwrap_or_default();
    let expected = only_child(reference, DSIG, "DigestValue").map(text_of).ok_or("its signature has no digest")?;
    let expected =
        STANDARD.decode(expected.split_whitespace().collect::<String>()).map_err(|_| "its digest isn't base64")?;
    let content = exc_c14n(text, element, Some(signature.id()), &inclusive, false);
    let digest = match digest_method {
        "http://www.w3.org/2001/04/xmlenc#sha256" => Sha256::digest(content.as_bytes()).to_vec(),
        "http://www.w3.org/2001/04/xmldsig-more#sha384" => Sha384::digest(content.as_bytes()).to_vec(),
        "http://www.w3.org/2001/04/xmlenc#sha512" => Sha512::digest(content.as_bytes()).to_vec(),
        _ => return Err("its digest must be SHA-256 or stronger (SHA-1 is refused)".into()),
    };
    if !crate::auth::constant_time_eq(&digest, &expected) {
        return Err("what it signed was changed".into());
    }

    let signed_info_inclusive: Vec<String> = only_child(signed_info, DSIG, "CanonicalizationMethod")
        .and_then(|m| m.children().find(|c| c.is_element() && c.tag_name().name() == "InclusiveNamespaces"))
        .and_then(|c| c.attribute("PrefixList"))
        .map(|list| list.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let message = exc_c14n(text, signed_info, None, &signed_info_inclusive, with_comments);
    let value = only_child(signature, DSIG, "SignatureValue").map(text_of).ok_or("its signature has no value")?;
    let value =
        STANDARD.decode(value.split_whitespace().collect::<String>()).map_err(|_| "its signature isn't base64")?;
    let (algorithm, value): (&dyn rustls_pki_types::SignatureVerificationAlgorithm, Vec<u8>) = match method {
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256" => (webpki::ring::RSA_PKCS1_2048_8192_SHA256, value),
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha384" => (webpki::ring::RSA_PKCS1_2048_8192_SHA384, value),
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha512" => (webpki::ring::RSA_PKCS1_2048_8192_SHA512, value),
        "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256" => {
            (webpki::ring::ECDSA_P256_SHA256, der_signature(&value)?)
        }
        "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha384" => {
            (webpki::ring::ECDSA_P384_SHA384, der_signature(&value)?)
        }
        _ => return Err("its signature must be RSA or ECDSA over SHA-256 or stronger (SHA-1 is refused)".into()),
    };
    for der in certificates {
        let der = CertificateDer::from(der.as_slice());
        let Ok(certificate) = webpki::EndEntityCert::try_from(&der) else { continue };
        if certificate.verify_signature(algorithm, message.as_bytes(), &value).is_ok() {
            return Ok(());
        }
    }
    Err("it isn't signed by the identity provider's certificate".into())
}

/// XML signatures write ECDSA as r and s side by side; ring reads ASN.1 DER.
fn der_signature(raw: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if raw.is_empty() || !raw.len().is_multiple_of(2) || raw.len() > 132 {
        return Err("its ECDSA signature is malformed".into());
    }
    let integer = |bytes: &[u8]| {
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len() - 1);
        let mut value = bytes[start..].to_vec();
        if value[0] & 0x80 != 0 {
            value.insert(0, 0);
        }
        let mut out = vec![0x02, value.len() as u8];
        out.extend(value);
        out
    };
    let (r, s) = raw.split_at(raw.len() / 2);
    let body = [integer(r), integer(s)].concat();
    let mut out = vec![0x30];
    if body.len() >= 0x80 {
        out.push(0x81);
    }
    out.push(body.len() as u8);
    out.extend(body);
    Ok(out)
}

/// This side's metadata, for providers that read it.
pub fn metadata(entity_id: &str, acs_url: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{entity}">
  <md:SPSSODescriptor AuthnRequestsSigned="false" WantAssertionsSigned="true" protocolSupportEnumeration="{PROTOCOL}">
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:unspecified</md:NameIDFormat>
    <md:AssertionConsumerService Binding="{POST_BINDING}" Location="{acs}" index="0" isDefault="true"/>
  </md:SPSSODescriptor>
</md:EntityDescriptor>
"#,
        entity = xml::escape(entity_id),
        acs = xml::escape(acs_url),
    )
}

#[cfg(test)]
mod tests {
    //! The responses in testdata/ were signed by signxml (lxml's
    //! canonicalization and OpenSSL), so these check fuwa against an
    //! implementation it shares nothing with.
    use super::*;

    const SP: &str = "https://chat.example.com/sso/servers/01J00000000000000000000000";
    const RSA: &str = include_str!("testdata/idp-rsa.crt");
    const EC: &str = include_str!("testdata/idp-ec.crt");

    fn provider(certificates: &str) -> Provider {
        Provider {
            protocol: super::super::Protocol::Saml,
            name: "Acme".into(),
            saml_entity_id: "https://idp.example.com".into(),
            saml_sso_url: "https://idp.example.com/sso".into(),
            saml_certificates: certificates.into(),
            ..Default::default()
        }
    }

    fn check(file: &str, certificates: &str, now: &str, request_id: &str) -> Result<Identity> {
        let xml = std::fs::read(format!("{}/src/sso/testdata/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let provider = provider(certificates);
        let (entity_id, acs_url) = (format!("{SP}/saml/metadata"), format!("{SP}/saml"));
        identify(
            &STANDARD.encode(xml),
            &Expect {
                provider: &provider,
                entity_id: &entity_id,
                acs_url: &acs_url,
                request_id,
                now: xml::parse_time(now).unwrap(),
            },
        )
    }

    fn ok(file: &str, certificates: &str) -> Result<Identity> {
        check(file, certificates, "2026-10-03T01:01:00Z", "_req1")
    }

    #[test]
    fn signed_assertions_and_responses_are_taken() {
        let identity = ok("assertion-signed.xml", RSA).unwrap();
        assert_eq!(identity.subject, "ana-123");
        assert_eq!(identity.email, "ana@acme.com");
        assert_eq!(identity.name, "Ana & Co");
        assert_eq!(ok("response-signed.xml", RSA).unwrap().subject, "ana-123");
        assert_eq!(ok("assertion-signed-ecdsa.xml", EC).unwrap().subject, "ana-123");
        // Any of the listed certificates will do, as when a provider rotates.
        assert!(ok("assertion-signed.xml", &format!("{EC}\n{RSA}")).is_ok());
    }

    #[test]
    fn unsigned_changed_or_wrapped_responses_are_refused() {
        for file in ["unsigned.xml", "tampered.xml", "wrapped.xml", "wrapped-new-id.xml", "other-signer.xml"] {
            let err = ok(file, RSA).unwrap_err().to_string();
            assert!(err.contains("refused"), "{file}: {err}");
        }
        assert!(ok("assertion-signed.xml", EC).is_err(), "signed with another key");
    }

    #[test]
    fn responses_hold_only_for_their_request_and_time() {
        assert!(check("assertion-signed.xml", RSA, "2026-10-03T01:01:00Z", "_another").is_err());
        assert!(check("assertion-signed.xml", RSA, "2026-10-03T01:20:00Z", "_req1").is_err(), "ran out");
        assert!(check("assertion-signed.xml", RSA, "2026-10-03T00:50:00Z", "_req1").is_err(), "not yet");
        // A little clock skew is allowed either way.
        assert!(check("assertion-signed.xml", RSA, "2026-10-03T01:11:00Z", "_req1").is_ok());
        assert!(check("assertion-signed.xml", RSA, "2026-10-03T00:57:00Z", "_req1").is_ok());
    }

    #[test]
    fn requests_deflate_and_carry_the_relay_state() {
        let provider = provider(RSA);
        let (url, id) = authorize_url(&provider, "https://sp/meta", "https://sp/acs", "st", 0).unwrap();
        let url = reqwest::Url::parse(&url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["RelayState"], "st");
        let deflated = STANDARD.decode(&query["SAMLRequest"]).unwrap();
        let mut inflated = String::new();
        std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(deflated.as_slice()), &mut inflated)
            .unwrap();
        assert!(
            inflated.contains(&format!("ID=\"{id}\""))
                && inflated.contains("<saml:Issuer>https://sp/meta</saml:Issuer>")
        );
        assert!(inflated.contains("AssertionConsumerServiceURL=\"https://sp/acs\""));
    }

    #[test]
    fn ecdsa_signatures_become_der() {
        let raw = [[0x80u8; 32], [0x01; 32]].concat();
        let der = der_signature(&raw).unwrap();
        assert_eq!(der[0], 0x30);
        assert_eq!(&der[2..5], &[0x02, 33, 0x00]);
    }
}
