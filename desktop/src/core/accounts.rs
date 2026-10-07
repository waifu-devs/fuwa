//! Several accounts on one instance, like the web app's account switcher
//! (`fuwa/saved.ts`, `components/AccountSwitcher.tsx`): one is in use (its
//! engine is connected), the others are kept with their sessions to switch
//! to without signing in again. Each account's encrypted messages keep their
//! own vault (`vault::Vault::dir_for` is per account), so switching never
//! mixes them.

use crate::core::Core;
use crate::core::config::SavedAccount;
use crate::pb;

/// An account's card, as kept: from the account itself and its session.
pub fn card_of(user: &pb::User, token: &str) -> SavedAccount {
    SavedAccount {
        user_id: user.id.clone(),
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        avatar_url: user.avatar_url.clone(),
        token: Some(token.to_owned()),
    }
}

/// Keeps `card`: in place of the same account, or a session from before
/// accounts were told apart that has the same token; new ones go last.
pub fn with_card(list: &mut Vec<SavedAccount>, card: SavedAccount) {
    match list.iter_mut().find(|a| a.user_id == card.user_id) {
        Some(a) => *a = card,
        None => {
            list.retain(|a| a.token != card.token);
            list.push(card);
        }
    }
}

/// A username as streamer mode shows it: "shixzie" as "s••••••", the first
/// letter and a dot for each other one (3 to 10), as the web's `maskName`.
pub fn mask_name(name: &str) -> String {
    let mut letters = name.chars();
    let first = letters.next().map(String::from).unwrap_or_default();
    first + &"•".repeat(letters.count().clamp(3, 10))
}

/// As a `pb::User`, for drawing an account that isn't connected.
pub fn user_of(a: &SavedAccount) -> pb::User {
    pb::User {
        id: a.user_id.clone(),
        username: a.username.clone(),
        display_name: a.display_name.clone(),
        avatar_url: a.avatar_url.clone(),
        ..Default::default()
    }
}

impl Core {
    /// The accounts kept on an instance, the one in use among them.
    pub fn kept_accounts(&self, key: &str) -> Vec<SavedAccount> {
        self.kept.lock().get(key).cloned().unwrap_or_default()
    }

    /// The instance said whose session this is: keep its card (and token) to switch back to.
    pub(crate) fn remember_account(&self, key: &str, user: &pb::User, token: &str) {
        {
            let mut kept = self.kept.lock();
            let list = kept.entry(key.to_owned()).or_default();
            let card = card_of(user, token);
            if list.contains(&card) {
                return;
            }
            with_card(list, card);
        }
        self.persist();
    }

    /// Forgets one kept account here (signing out of it, or its session ending).
    pub(crate) fn forget_account(&self, key: &str, user_id: &str) {
        let changed = {
            let mut kept = self.kept.lock();
            let list = kept.entry(key.to_owned()).or_default();
            let before = list.len();
            list.retain(|a| a.user_id != user_id);
            before != list.len()
        };
        if changed {
            self.persist();
        }
    }

    /// Carries on as another account kept on this instance: its session
    /// connects in place of the one in use, which stays kept.
    pub fn switch_account(self: &std::sync::Arc<Self>, key: &str, user_id: &str) -> bool {
        let Some(api) = self.api(key) else { return false };
        let Some(token) = self.kept_accounts(key).into_iter().find(|a| a.user_id == user_id).and_then(|a| a.token)
        else {
            return false;
        };
        if api.token().as_deref() == Some(token.as_str()) {
            return true;
        }
        self.add_instance(&api.url, Some(token));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str) -> pb::User {
        pb::User { id: id.into(), username: id.into(), ..Default::default() }
    }

    #[test]
    fn cards_replace_their_own_account_and_sessions_from_before() {
        let mut list = vec![SavedAccount { token: Some("old".into()), ..Default::default() }];
        with_card(&mut list, card_of(&user("a"), "old"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].user_id, "a");
        with_card(&mut list, card_of(&user("b"), "t2"));
        with_card(&mut list, card_of(&pb::User { display_name: "Alice".into(), ..user("a") }, "t3"));
        assert_eq!(list.iter().map(|a| a.user_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!((list[0].display_name.as_str(), list[0].token.as_deref()), ("Alice", Some("t3")));
    }

    #[test]
    fn names_hide_all_but_their_first_letter() {
        assert_eq!(mask_name("shixzie"), "s••••••");
        assert_eq!(mask_name("al"), "a•••");
        assert_eq!(mask_name("averyveryverylongname"), "a••••••••••");
    }
}
