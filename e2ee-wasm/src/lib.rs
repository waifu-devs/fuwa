//! fuwa-e2ee for the web app: a [`Device`] JavaScript can hold. `pnpm run
//! wasm` in web/ builds it (cargo for wasm32-unknown-unknown, then
//! wasm-bindgen) into `web/src/e2ee/pkg`, where the app imports it from.
//!
//! Errors are thrown as `Error`s whose `name` says what went wrong:
//! "NotMember", "Intruder", "Behind", "Invalid" or "Mls".

use fuwa_e2ee::{Commit, Error, Member, Processed};
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Device(fuwa_e2ee::Device);

fn thrown(err: Error) -> JsValue {
    let name = match err {
        Error::Invalid(_) => "Invalid",
        Error::NotMember => "NotMember",
        Error::Intruder(_) => "Intruder",
        Error::Behind => "Behind",
        Error::Mls(_) => "Mls",
    };
    let error = js_sys::Error::new(&err.to_string());
    error.set_name(name);
    error.into()
}

type Result<T> = std::result::Result<T, JsValue>;

/// A plain object from key and value pairs.
fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let object = Object::new();
    for (key, value) in fields {
        // Setting a property on a plain object can't fail.
        let _ = Reflect::set(&object, &JsValue::from_str(key), value);
    }
    object.into()
}

fn bytes(value: &[u8]) -> JsValue {
    Uint8Array::from(value).into()
}

fn member(member: &Member) -> JsValue {
    object(&[
        ("userId", member.user_id.as_str().into()),
        ("deviceId", member.device_id.as_str().into()),
        ("signatureKey", bytes(&member.signature_key)),
    ])
}

fn members(list: &[Member]) -> JsValue {
    list.iter().map(member).collect::<Array>().into()
}

fn commit(commit: Commit) -> JsValue {
    object(&[
        ("commit", bytes(&commit.commit)),
        ("groupInfo", bytes(&commit.group_info)),
        ("welcome", commit.welcome.as_deref().map(bytes).unwrap_or(JsValue::UNDEFINED)),
        ("added", members(&commit.added)),
        ("removed", members(&commit.removed)),
    ])
}

fn get(value: &JsValue, key: &str) -> Result<JsValue> {
    Reflect::get(value, &JsValue::from_str(key))
}

#[wasm_bindgen]
impl Device {
    /// A new device for an account, with a new signing key.
    #[wasm_bindgen(constructor)]
    pub fn new(user_id: &str) -> Result<Device> {
        fuwa_e2ee::Device::new(user_id).map(Device).map_err(thrown)
    }

    /// A device as `save` left it.
    pub fn restore(saved: &[u8]) -> Result<Device> {
        fuwa_e2ee::Device::restore(saved).map(Device).map_err(thrown)
    }

    /// Everything the device knows. Keep it before sending anything it made.
    pub fn save(&self) -> Vec<u8> {
        self.0.save()
    }

    #[wasm_bindgen(getter, js_name = userId)]
    pub fn user_id(&self) -> String {
        self.0.user_id().to_owned()
    }

    #[wasm_bindgen(getter, js_name = signatureKey)]
    pub fn signature_key(&self) -> Vec<u8> {
        self.0.signature_key().to_vec()
    }

    #[wasm_bindgen(getter, js_name = deviceId)]
    pub fn device_id(&self) -> String {
        self.0.device_id()
    }

    /// Key packages others add this device with: an array of Uint8Arrays.
    #[wasm_bindgen(js_name = keyPackages)]
    pub fn key_packages(&self, count: u32) -> Result<Array> {
        let packages = self.0.key_packages(count as usize).map_err(thrown)?;
        Ok(packages.iter().map(|p| bytes(p)).collect())
    }

    #[wasm_bindgen(js_name = lastResortKeyPackage)]
    pub fn last_resort_key_package(&self) -> Result<Vec<u8>> {
        self.0.last_resort_key_package().map_err(thrown)
    }

    #[wasm_bindgen(js_name = isMember)]
    pub fn is_member(&self, conversation_id: &str) -> bool {
        self.0.is_member(conversation_id)
    }

    pub fn epoch(&self, conversation_id: &str) -> Result<f64> {
        self.0.epoch(conversation_id).map(|epoch| epoch as f64).map_err(thrown)
    }

    /// `{ epoch, secret }`: the group's exported secret at its current
    /// epoch (see `Device::export_secret`).
    #[wasm_bindgen(js_name = exportSecret)]
    pub fn export_secret(&self, conversation_id: &str, label: &str, length: u32) -> Result<JsValue> {
        let (epoch, secret) = self.0.export_secret(conversation_id, label, length as usize).map_err(thrown)?;
        Ok(object(&[("epoch", JsValue::from_f64(epoch as f64)), ("secret", bytes(&secret))]))
    }

    #[wasm_bindgen(js_name = hasPendingCommit)]
    pub fn has_pending_commit(&self, conversation_id: &str) -> Result<bool> {
        self.0.has_pending_commit(conversation_id).map_err(thrown)
    }

    /// Every device in the conversation: `{ userId, deviceId, signatureKey }`.
    pub fn members(&self, conversation_id: &str) -> Result<JsValue> {
        self.0.members(conversation_id).map(|list| members(&list)).map_err(thrown)
    }

    #[wasm_bindgen(js_name = createGroup)]
    pub fn create_group(&self, conversation_id: &str) -> Result<()> {
        self.0.create_group(conversation_id).map_err(thrown)
    }

    pub fn forget(&self, conversation_id: &str) -> Result<()> {
        self.0.forget(conversation_id).map_err(thrown)
    }

    /// A commit adding devices (`[{ deviceId, keyPackage }]`) and removing
    /// others (device ids): `{ commit, groupInfo, welcome?, added, removed }`.
    pub fn commit(
        &self,
        conversation_id: &str,
        adds: Array,
        removes: Vec<String>,
        allowed: Vec<String>,
    ) -> Result<JsValue> {
        let mut packages = Vec::with_capacity(adds.length() as usize);
        for add in adds.iter() {
            let device_id = get(&add, "deviceId")?.as_string().ok_or("deviceId must be a string")?;
            let key_package = Uint8Array::new(&get(&add, "keyPackage")?).to_vec();
            packages.push((device_id, key_package));
        }
        self.0.commit(conversation_id, &packages, &removes, &allowed).map(commit).map_err(thrown)
    }

    #[wasm_bindgen(js_name = discardPending)]
    pub fn discard_pending(&self, conversation_id: &str) -> Result<()> {
        self.0.discard_pending(conversation_id).map_err(thrown)
    }

    /// Joins from a welcome; returns the epoch it joined at.
    #[wasm_bindgen(js_name = joinFromWelcome)]
    pub fn join_from_welcome(&self, conversation_id: &str, welcome: &[u8], allowed: Vec<String>) -> Result<f64> {
        self.0.join_from_welcome(conversation_id, welcome, &allowed).map(|epoch| epoch as f64).map_err(thrown)
    }

    /// Joins by itself from a group info; returns the commit to send.
    #[wasm_bindgen(js_name = joinByItself)]
    pub fn join_by_itself(&self, conversation_id: &str, group_info: &[u8], allowed: Vec<String>) -> Result<JsValue> {
        self.0.join_by_itself(conversation_id, group_info, &allowed).map(commit).map_err(thrown)
    }

    pub fn encrypt(&self, conversation_id: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.0.encrypt(conversation_id, plaintext).map_err(thrown)
    }

    /// Signs a payload with this device's signature key (see `verify`).
    pub fn sign(&self, payload: &[u8]) -> Result<Vec<u8>> {
        self.0.sign(payload).map_err(thrown)
    }

    /// Opens the next record: `{ kind: "message", sender, plaintext }`,
    /// `{ kind: "commit", by?, added, removed, removedMe }`, `{ kind: "stale" }`
    /// or `{ kind: "own" }`.
    pub fn process(&self, conversation_id: &str, record: &[u8], own: bool, allowed: Vec<String>) -> Result<JsValue> {
        Ok(match self.0.process(conversation_id, record, own, &allowed).map_err(thrown)? {
            Processed::Message { sender, plaintext } => {
                object(&[("kind", "message".into()), ("sender", member(&sender)), ("plaintext", bytes(&plaintext))])
            }
            Processed::Commit { by, added, removed, removed_me } => object(&[
                ("kind", "commit".into()),
                ("by", by.as_ref().map(member).unwrap_or(JsValue::UNDEFINED)),
                ("added", members(&added)),
                ("removed", members(&removed)),
                ("removedMe", removed_me.into()),
            ]),
            Processed::Stale => object(&[("kind", "stale".into())]),
            Processed::Own => object(&[("kind", "own".into())]),
        })
    }
}

/// The SHA-256 of some bytes, as hex. Browsers only hash in secure contexts
/// (https), and a self-hosted instance may be plain http on a home network.
#[wasm_bindgen]
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether a device with this signing key signed the payload (`Device.sign`).
#[wasm_bindgen]
pub fn verify(signature_key: &[u8], payload: &[u8], signature: &[u8]) -> bool {
    fuwa_e2ee::verify(signature_key, payload, signature)
}

/// A device's id from its signing key.
#[wasm_bindgen(js_name = deviceId)]
pub fn device_id(signature_key: &[u8]) -> String {
    fuwa_e2ee::device_id(signature_key)
}

/// The 60-digit safety number for two people, from each one's id and the
/// signing keys (Uint8Arrays) of their devices.
#[wasm_bindgen(js_name = safetyNumber)]
pub fn safety_number(a_user: &str, a_keys: Array, b_user: &str, b_keys: Array) -> String {
    let keys = |list: Array| list.iter().map(|key| Uint8Array::new(&key).to_vec()).collect::<Vec<_>>();
    let (a_keys, b_keys) = (keys(a_keys), keys(b_keys));
    fuwa_e2ee::safety_number((a_user, &a_keys), (b_user, &b_keys))
}
