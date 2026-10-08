//! What each right-click menu holds, like the web app's `components/menus/`:
//! sections in the same order (what was clicked on, primary, notifications,
//! manage, moderate, developer, danger), each item only where its button
//! would be. "Copy … ID" items show with Developer Mode on.

use gpui_kit::{App, Context, Hsla};

use crate::core::account::NotificationPatch;
use crate::core::dms::now_ms;
use crate::core::i18n::t;
use crate::core::moderation::Action;
use crate::core::store::InstanceState;
use crate::pb::{self, NotificationLevel as Level, Permission as P};
use crate::ui::app::{Dialog, FuwaApp, Menu, Target};
use crate::ui::context_menu::{Built, Item, MenuOf, copy, run};
use crate::ui::menus::{MUTE_FOR, muted_label};

impl FuwaApp {
    pub(crate) fn context_items(&self, of: &MenuOf, cx: &App) -> Built {
        let developer = self.prefs.developer_mode;
        let copy_id = |id: &str, what: &'static str| -> Vec<Item> {
            if !developer || id.is_empty() {
                return Vec::new();
            }
            let id = id.to_owned();
            vec![Item::act(
                format!("Copy {what} ID"),
                "binary",
                run(move |this, _, cx| copy(this, id.clone(), &format!("{what} ID"), cx)),
            )]
        };
        match of {
            MenuOf::Message { msg, thread, picture, selection } => {
                let m = msg.clone();
                let mut target = Vec::new();
                if !selection.is_empty() {
                    let text = selection.clone();
                    target.push(Item::act(
                        "Copy",
                        "copy",
                        run(move |this, _, cx| copy(this, text.clone(), "text", cx)),
                    ));
                }
                if let Some(file) = picture.and_then(|n| m.attachments.get(n)).cloned() {
                    let key = self.target().map(|t| t.key().to_owned()).unwrap_or_default();
                    let open = Dialog::Picture {
                        key: key.clone(),
                        url: file.url.clone(),
                        name: file.filename.clone(),
                        width: file.width,
                        height: file.height,
                        bytes: file.size,
                    };
                    let (url, name, bytes) = (file.url.clone(), file.filename.clone(), file.size);
                    target.push(Item::act(
                        "Open picture",
                        "image",
                        run(move |this, window, cx| this.open_dialog(open.clone(), window, cx)),
                    ));
                    target.push(Item::act(
                        "Save picture",
                        "download",
                        run(move |this, _, cx| this.save_file(key.clone(), url.clone(), name.clone(), bytes, cx)),
                    ));
                    let link = file.url.clone();
                    target.push(Item::act(
                        "Copy picture link",
                        "link",
                        run(move |this, _, cx| copy(this, link.clone(), "picture link", cx)),
                    ));
                }
                // A message still sending, or that couldn't be: try again, copy it, or let it go.
                if m.pending {
                    let mut items = Vec::new();
                    if m.failed.is_some() {
                        let (nonce, thread) = (m.nonce, thread.clone());
                        items.push(Item::act(
                            "Retry",
                            "rotate-ccw",
                            run(move |this, _, cx| this.retry(nonce, thread.clone(), cx)),
                        ));
                    }
                    if !m.content.is_empty() {
                        let text = m.content.clone();
                        items.push(Item::act(
                            "Copy text",
                            "copy",
                            run(move |this, _, cx| copy(this, text.clone(), "text", cx)),
                        ));
                    }
                    let mut danger = Vec::new();
                    if m.failed.is_some() {
                        let (nonce, thread) = (m.nonce, thread.clone());
                        danger.push(
                            Item::act(
                                "Dismiss",
                                "x",
                                run(move |this, _, _| {
                                    if let Some(Target::Channel { key, channel, .. }) = this.target() {
                                        let at = thread.as_deref().map_or(channel, crate::core::threads::thread_key);
                                        this.core.dismiss_pending(&key, &at, nonce);
                                    }
                                }),
                            )
                            .danger(),
                        );
                    }
                    return Built::of(vec![target, items, danger]);
                }
                let can_thread = m.thread.can_thread && !m.editing && !m.keeping_out;
                let can_edit = m.mine && !m.unreadable && !m.editing && m.poll.is_none() && m.voice.is_none();
                let mut primary = Vec::new();
                if can_thread {
                    let id = m.id.clone();
                    let open = m.thread.replies.is_some();
                    primary.push(Item::act(
                        if open { "Open thread" } else { "Reply in thread" },
                        "message-square-reply",
                        run(move |this, window, cx| this.open_thread(id.clone(), window, cx)),
                    ));
                }
                if can_edit {
                    let (id, in_thread) = (m.id.clone(), thread.is_some());
                    primary.push(Item::act(
                        "Edit message",
                        "pencil",
                        run(move |this, window, cx| {
                            this.start_edit(id.clone(), window, cx);
                            this.edit_in_thread = in_thread;
                            this.sync_list(cx);
                            this.sync_thread(cx);
                        }),
                    ));
                }
                if m.can_pin && !m.editing {
                    let (id, pinned, in_thread) = (m.id.clone(), m.pinned, thread.is_some());
                    primary.push(Item::act(
                        if pinned { t("chattools.pins.unpinMessage") } else { t("chattools.pins.pin") },
                        if pinned { "pin-off" } else { "pin" },
                        run(move |this, _, cx| this.toggle_pin(id.clone(), !pinned, in_thread, cx)),
                    ));
                }
                if !m.unreadable && !m.content.trim().is_empty() {
                    let text = m.content.clone();
                    primary.push(Item::act(
                        "Copy text",
                        "copy",
                        run(move |this, _, cx| copy(this, text.clone(), "text", cx)),
                    ));
                }
                let mut manage = Vec::new();
                if m.keep_out && !m.editing && !m.keeping_out {
                    let id = m.id.clone();
                    manage.push(
                        Item::act(
                            format!("Keep {} out", m.name),
                            "user-x",
                            run(move |this, _, cx| {
                                this.keeping_out = Some(id.clone());
                                this.sync_list(cx);
                            }),
                        )
                        .danger(),
                    );
                }
                // Private messages are numbered in their conversation: they have no ID to copy.
                let in_server = matches!(self.target(), Some(Target::Channel { .. }));
                let developer = if in_server { copy_id(&m.id, "message") } else { Vec::new() };
                let mut danger = Vec::new();
                if m.can_delete && !m.editing && !m.keeping_out {
                    let id = m.id.clone();
                    danger.push(
                        Item::act("Delete message", "trash", run(move |this, _, cx| this.delete(id.clone(), cx)))
                            .danger(),
                    );
                }
                Built::of(vec![target, primary, manage, developer, danger])
            }
            MenuOf::Member { key, server, user_id } => {
                self.member_items(key, server.as_deref(), user_id, copy_id(user_id, "user"), cx)
            }
            MenuOf::Channel { key, server, channel } => {
                self.channel_items(key, server, channel, copy_id(channel, "channel"))
            }
            MenuOf::Category { key, server, category } => {
                self.category_items(key, server, category, copy_id(category, "category"))
            }
            MenuOf::Server { key, server } => self.server_items(key, server, copy_id(server, "server")),
            MenuOf::Applied { key, server } => self.applied_items(key, server),
            MenuOf::ServerHeader { key, server } => self.server_header_items(key, server, copy_id(server, "server")),
            MenuOf::LiveTile { key, server, tile } => self.live_tile_items(key, server, tile),
            MenuOf::RailFolder { key, folder } => self.rail_folder_items(key, folder),
            MenuOf::RailAdd => self.rail_add_items(),
            MenuOf::Dm { key, conversation } => {
                let (unread, other) = self.core.shared.read(|s| {
                    let Some(i) = s.instance(key) else { return (0, String::new()) };
                    let me = i.me.as_ref().map(|u| u.id.clone()).unwrap_or_default();
                    let other = i
                        .dms
                        .conversations
                        .iter()
                        .find(|c| c.id == *conversation)
                        .and_then(|c| c.users.iter().find(|u| u.id != me).map(|u| u.id.clone()))
                        .unwrap_or_default();
                    (i.dms.unread.get(conversation).copied().unwrap_or(0), other)
                });
                let (k, id) = (key.clone(), conversation.clone());
                let read = Item::act(
                    "Mark as read",
                    "check-check",
                    run(move |this, _, cx| {
                        this.core.mark_read(&k, std::slice::from_ref(&id));
                        cx.notify();
                    }),
                )
                .disabled(unread == 0);
                Built::of(vec![vec![read], copy_id(&other, "user")])
            }
            MenuOf::Composer => self.composer_items(cx),
        }
    }

    fn member_items(&self, key: &str, server: Option<&str>, user_id: &str, developer: Vec<Item>, cx: &App) -> Built {
        struct Facts {
            me: bool,
            agent: bool,
            dms: bool,
            username: String,
            member: Option<pb::Member>,
            roles: Vec<pb::Role>,
            allowed: Vec<P>,
            /// You may change their nickname.
            rename: bool,
            /// Timed out now.
            timed_out: bool,
            /// Where you stand with them, when this instance has friends.
            friend: Option<i32>,
        }
        let facts = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let me = i.me.as_ref().is_some_and(|me| me.id == user_id);
            let member = server.and_then(|sv| {
                i.members.get(sv)?.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)).cloned()
            });
            let user = member.as_ref().and_then(|m| m.user.clone()).or_else(|| known_user(i, user_id));
            let (roles, allowed) = match server {
                Some(sv) if member.is_some() => {
                    let access = i.access(sv);
                    let roles = if access.has(P::ManageRoles) {
                        let mut list: Vec<pb::Role> = i
                            .roles
                            .get(sv)
                            .into_iter()
                            .flatten()
                            .filter(|r| r.id != sv && access.above(r.position))
                            .cloned()
                            .collect();
                        list.sort_by_key(|r| std::cmp::Reverse(r.position));
                        list
                    } else {
                        Vec::new()
                    };
                    (roles, i.can_moderate(sv, user_id))
                }
                _ => (Vec::new(), Vec::new()),
            };
            let rename = server.is_some_and(|sv| member.is_some() && i.can_rename(sv, user_id));
            let timed_out = member
                .as_ref()
                .is_some_and(|m| crate::core::moderation::timed_out_until(m, crate::core::dms::now_ms()).is_some());
            Some(Facts {
                me,
                agent: crate::ui::widgets::is_agent(user.as_ref()),
                dms: matches!(i.dms.status, crate::core::dms::DmStatus::Ready | crate::core::dms::DmStatus::Starting),
                username: user.map(|u| u.username).unwrap_or_default(),
                member,
                rename,
                timed_out,
                roles,
                allowed,
                friend: matches!(
                    i.friends.status,
                    crate::core::friends::FriendsStatus::Ready | crate::core::friends::FriendsStatus::Loading
                )
                .then(|| crate::core::friends::state_with(&i.friends.list, user_id, crate::core::dms::now_ms())),
            })
        });
        let Some(f) = facts else { return Built::of(Vec::new()) };
        let mut primary = Vec::new();
        {
            let (k, uid, sv) = (key.to_owned(), user_id.to_owned(), server.map(str::to_owned));
            primary.push(Item::act(
                crate::core::i18n::t("workspace.menu.member.profile"),
                "user-round",
                run(move |this, window, cx| {
                    let dialog = Dialog::Profile { key: k.clone(), user_id: uid.clone(), server: sv.clone() };
                    this.open_dialog(dialog, window, cx)
                }),
            ));
        }
        if !f.me && !f.agent && f.dms && f.friend != Some(crate::core::friends::BLOCKED) {
            let (k, uid) = (key.to_owned(), user_id.to_owned());
            primary.push(
                Item::act(
                    crate::core::i18n::t("workspace.menu.member.message"),
                    "message-circle",
                    run(move |this, window, cx| this.message_person(k.clone(), uid.clone(), window, cx)),
                )
                .hint(crate::core::i18n::t("workspace.menu.member.encrypted")),
            );
        }
        // Mention goes in the message box of the server's channel you're in.
        let here = matches!(self.target(), Some(Target::Channel { key: k, server: s, .. }) if k == key && Some(s.as_str()) == server);
        if here && !f.username.is_empty() {
            let text = format!("@{} ", f.username);
            primary.push(Item::act(
                crate::core::i18n::t("workspace.menu.member.mention"),
                "at-sign",
                run(move |this, window, cx| {
                    let text = text.clone();
                    this.composer.update(cx, |state, cx| {
                        state.focus(window, cx);
                        state.replace(text, window, cx);
                    });
                }),
            ));
        }
        let mut manage = Vec::new();
        if f.me && server.is_some() {
            manage.push(Item::act(
                crate::core::i18n::t("workspace.menu.member.editServerProfile"),
                "id-card",
                run(|this, window, cx| {
                    this.open_settings(window, cx);
                    if let Some(view) = &this.settings {
                        view.update(cx, |view, cx| {
                            view.page = crate::ui::settings::Page::ServerProfiles;
                            cx.notify();
                        });
                    }
                }),
            ));
        }
        if let (Some(sv), true) = (server, f.rename) {
            let (k, s, uid) = (key.to_owned(), sv.to_owned(), user_id.to_owned());
            manage.push(Item::act(
                crate::core::i18n::t("workspace.menu.member.nickname"),
                "pencil",
                run(move |this, window, cx| {
                    let dialog = Dialog::Moderate {
                        key: k.clone(),
                        server: s.clone(),
                        user_id: uid.clone(),
                        action: Action::Nickname,
                    };
                    this.open_dialog(dialog, window, cx)
                }),
            ));
        }
        if let (Some(sv), Some(member)) = (server, &f.member)
            && !f.roles.is_empty()
        {
            let held = &member.role_ids;
            let count = held.iter().filter(|r| r.as_str() != sv).count();
            let items = f
                .roles
                .iter()
                .map(|role| {
                    let on = held.contains(&role.id);
                    let (k, s, uid, rid) = (key.to_owned(), sv.to_owned(), user_id.to_owned(), role.id.clone());
                    let color: Option<Hsla> = role.color.map(|c| gpui_kit::rgb(c as u32).into());
                    Item::check(
                        role.name.clone(),
                        on,
                        false,
                        run(move |this, _, cx| {
                            let (core, k, s, uid, rid) =
                                (this.core.clone(), k.clone(), s.clone(), uid.clone(), rid.clone());
                            this.run(
                                cx,
                                async move { core.set_member_role(&k, &s, &uid, &rid, !on).await },
                                |this, result, cx| {
                                    if let Err(err) = result {
                                        this.toast(
                                            "circle-alert",
                                            "Couldn't change their roles".into(),
                                            err.message,
                                            None,
                                            None,
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                },
                            );
                        }),
                    )
                    .colored(color)
                    .keep_open()
                })
                .collect();
            manage.push(
                Item::sub(crate::core::i18n::t("workspace.menu.member.roles"), "shield", items).hint(if count > 0 {
                    count.to_string()
                } else {
                    String::new()
                }),
            );
        }
        let mut moderate = Vec::new();
        if let Some(sv) = server {
            for permission in &f.allowed {
                let ending = *permission == P::TimeOutMembers && f.timed_out;
                let (label, glyph, action) = match permission {
                    P::TimeOutMembers if ending => {
                        ("workspace.menu.member.endTimeout", "timer-off", Action::TimeOut(3_600))
                    }
                    P::TimeOutMembers => ("workspace.menu.member.timeout", "hourglass", Action::TimeOut(3_600)),
                    P::KickMembers => ("workspace.menu.member.kick", "door-open", Action::Kick),
                    _ => ("workspace.menu.member.ban", "gavel", Action::Ban(0)),
                };
                let (k, s, uid) = (key.to_owned(), sv.to_owned(), user_id.to_owned());
                let item = Item::act(
                    crate::core::i18n::t(label),
                    glyph,
                    run(move |this, window, cx| {
                        let dialog =
                            Dialog::Moderate { key: k.clone(), server: s.clone(), user_id: uid.clone(), action };
                        this.open_dialog(dialog, window, cx)
                    }),
                );
                moderate.push(if ending { item } else { item.danger() });
            }
        }
        let mut friend = Vec::new();
        if let Some(state) = f.friend.filter(|_| !f.me && !f.agent) {
            use crate::core::friends::{BLOCKED, FRIEND, INCOMING, OUTGOING};
            use crate::ui::friends::Act;
            let item = |label: &str, glyph: &'static str, act: Act| {
                let (k, uid) = (key.to_owned(), user_id.to_owned());
                Item::act(label, glyph, run(move |this, _, cx| this.friend_act(&k, &uid, act, cx)))
            };
            match state {
                0 => friend.push(item(&crate::core::i18n::t("dms-calls.friends.add"), "user-plus", Act::Request)),
                OUTGOING => friend.push(item(
                    &crate::core::i18n::t("dms-calls.friends.actions.cancelRequest"),
                    "x",
                    Act::Remove,
                )),
                INCOMING => {
                    friend.push(item(
                        &crate::core::i18n::t("dms-calls.friends.actions.accept"),
                        "user-check",
                        Act::Accept,
                    ));
                    friend.push(item(&crate::core::i18n::t("dms-calls.friends.actions.decline"), "x", Act::Remove));
                }
                FRIEND => {
                    friend.push(item(&crate::core::i18n::t("dms-calls.friends.remove"), "user-minus", Act::Remove))
                }
                _ => {}
            }
            if state == BLOCKED {
                friend.push(item(&crate::core::i18n::t("dms-calls.friends.unblock"), "shield-off", Act::Unblock));
            } else {
                friend.push(
                    item(&crate::core::i18n::t("dms-calls.friends.page.block"), "ban", Act::Block).danger().confirm(
                        format!("Block @{}?", f.username),
                        "They can't message you, call you or send you requests, and they aren't told.",
                        "Block",
                    ),
                );
            }
        }
        let _ = cx;
        Built::of(vec![primary, friend, manage, moderate, developer])
    }

    fn channel_items(&self, key: &str, server: &str, channel: &str, developer: Vec<Item>) -> Built {
        let Some((c, access, unread, copies, link)) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let c = i.channel(server, channel)?.clone();
            let access = i.access(server);
            let unread = i.unread.get(channel).copied().unwrap_or(0)
                + crate::core::threads::channel_thread_unread(i, channel)
                + i.dms.unread.get(channel).copied().unwrap_or(0);
            let copies = may_duplicate(i, server, &c);
            let link = place_link(i, &[server, channel]);
            Some((c, access, unread, copies, link))
        }) else {
            return Built::of(Vec::new());
        };
        let kind = pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text);
        let texty = matches!(kind, pb::ChannelType::Text | pb::ChannelType::Announcement | pb::ChannelType::Secure);
        let name = if kind == pb::ChannelType::Voice { c.name.clone() } else { format!("#{}", c.name) };
        let mut primary = Vec::new();
        if texty {
            let (k, id) = (key.to_owned(), channel.to_owned());
            primary.push(
                Item::act(
                    "Mark as read",
                    "check-check",
                    run(move |this, _, cx| {
                        this.core.mark_read(&k, std::slice::from_ref(&id));
                        cx.notify();
                    }),
                )
                .disabled(unread == 0),
            );
        }
        if access.has_in(channel, P::CreateInvite) || access.has(P::CreateInvite) {
            primary.push(self.invite_item(key, server));
        }
        primary.push(Item::act(
            "Copy link",
            "link",
            run(move |this, _, cx| copy(this, link.clone(), "channel link", cx)),
        ));
        let notifications = if texty { self.notification_items(key, server, channel) } else { Vec::new() };
        let manages = access.has_in(channel, P::ManageChannels);
        let roles = access.has_in(channel, P::ManageRoles);
        let mut manage = Vec::new();
        if manages || roles {
            manage.push(self.edit_channel_item("Edit channel", key, server, channel, false));
        }
        if roles {
            manage.push(self.edit_channel_item("Permissions", key, server, channel, true));
        }
        if copies {
            let (k, s, original) = (key.to_owned(), server.to_owned(), c.clone());
            let shown = name.clone();
            manage.push(Item::act(
                "Duplicate channel",
                "copy-plus",
                run(move |this, _, cx| {
                    let (core, k, s, original, shown) =
                        (this.core.clone(), k.clone(), s.clone(), original.clone(), shown.clone());
                    this.run(
                        cx,
                        async move { core.duplicate_channel(&k, &s, &original).await },
                        move |this, result, cx| {
                            match result {
                                Ok(_) => this.toast(
                                    "copy-plus",
                                    format!("Made a copy of {shown}"),
                                    String::new(),
                                    None,
                                    None,
                                    cx,
                                ),
                                Err(err) => this.toast(
                                    "circle-alert",
                                    "Couldn't copy that channel".into(),
                                    err.message,
                                    None,
                                    None,
                                    cx,
                                ),
                            }
                            cx.notify();
                        },
                    );
                }),
            ));
        }
        let mut danger = Vec::new();
        if manages {
            danger.push(self.delete_channel_item(
                "Delete channel",
                key,
                server,
                channel,
                &name,
                "Every message in it goes too, for everyone. This can't be undone.",
            ));
        }
        Built::of(vec![primary, notifications, manage, developer, danger])
    }

    fn category_items(&self, key: &str, server: &str, category: &str, developer: Vec<Item>) -> Built {
        let Some((name, access, children, unread)) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let name = i.channel(server, category)?.name.clone();
            let children: Vec<String> =
                i.channels.get(server)?.iter().filter(|c| c.parent_id == category).map(|c| c.id.clone()).collect();
            let unread = children.iter().any(|id| {
                i.unread.get(id).is_some_and(|n| *n > 0) || crate::core::threads::channel_thread_unread(i, id) > 0
            });
            Some((name, i.access(server), children, unread))
        }) else {
            return Built::of(Vec::new());
        };
        let manages = access.has_in(category, P::ManageChannels);
        let roles = access.has_in(category, P::ManageRoles);
        let mut primary = Vec::new();
        {
            let k = key.to_owned();
            primary.push(
                Item::act(
                    "Mark as read",
                    "check-check",
                    run(move |this, _, cx| {
                        this.core.mark_read(&k, &children);
                        cx.notify();
                    }),
                )
                .disabled(!unread),
            );
        }
        if manages {
            let (k, s, parent) = (key.to_owned(), server.to_owned(), category.to_owned());
            primary.push(Item::act(
                "Create channel",
                "plus",
                run(move |this, window, cx| {
                    let dialog = Dialog::CreateChannel {
                        key: k.clone(),
                        server: s.clone(),
                        parent: parent.clone(),
                        kind: pb::ChannelType::Text,
                    };
                    this.open_dialog(dialog, window, cx)
                }),
            ));
        }
        let mut manage = Vec::new();
        if manages || roles {
            manage.push(self.edit_channel_item("Edit category", key, server, category, false));
        }
        if roles {
            manage.push(self.edit_channel_item("Permissions", key, server, category, true));
        }
        let mut danger = Vec::new();
        if manages {
            danger.push(self.delete_channel_item(
                "Delete category",
                key,
                server,
                category,
                &name,
                "Its channels stay, outside any category.",
            ));
        }
        Built::of(vec![primary, manage, developer, danger])
    }

    fn server_items(&self, key: &str, server: &str, developer: Vec<Item>) -> Built {
        let Some((access, unread, invite)) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            i.server(server)?;
            let access = i.access(server);
            let now = now_ms();
            let unread = i
                .channels
                .get(server)
                .into_iter()
                .flatten()
                .any(|c| i.unread.get(&c.id).is_some_and(|n| *n > 0) && !i.is_muted(server, &c.id, now));
            let invite =
                access.has(P::CreateInvite) || access.channels.keys().any(|c| access.has_in(c, P::CreateInvite));
            Some((access, unread, invite))
        }) else {
            return Built::of(Vec::new());
        };
        let mut primary = Vec::new();
        {
            let (k, s) = (key.to_owned(), server.to_owned());
            primary.push(
                Item::act(
                    "Mark as read",
                    "check-check",
                    run(move |this, _, cx| {
                        let ids: Vec<String> = this.core.shared.read(|st| {
                            st.instance(&k)
                                .and_then(|i| i.channels.get(&s))
                                .map(|l| l.iter().map(|c| c.id.clone()).collect())
                                .unwrap_or_default()
                        });
                        this.core.mark_read(&k, &ids);
                        cx.notify();
                    }),
                )
                .disabled(!unread),
            );
        }
        if invite {
            primary.push(self.invite_item(key, server));
        }
        let mut notifications = self.notification_items(key, server, "");
        {
            let menu = Menu::Server { key: key.to_owned(), server: server.to_owned() };
            notifications.push(Item::act(
                "Notification settings",
                "bell-ring",
                run(move |this, _, cx| {
                    this.menu = Some(menu.clone());
                    cx.notify();
                }),
            ));
        }
        let mut manage = Vec::new();
        if crate::ui::server_settings::can_open(&access) {
            let (k, s) = (key.to_owned(), server.to_owned());
            manage.push(Item::act(
                "Server settings",
                "settings",
                run(move |this, window, cx| this.open_server_settings(&k, &s, window, cx)),
            ));
        }
        let mut danger = Vec::new();
        if !access.owner {
            let (k, s) = (key.to_owned(), server.to_owned());
            danger.push(
                Item::act(
                    "Leave server",
                    "door-open",
                    run(move |this, window, cx| {
                        this.open_dialog(Dialog::LeaveServer { key: k.clone(), server: s.clone() }, window, cx)
                    }),
                )
                .danger(),
            );
        }
        // Your folders sit with the server's own settings, before its ID and Leave (rail.rs).
        let folder = self.rail_server_items(key, server);
        Built::of(vec![primary, notifications, manage, folder, developer, danger])
    }

    /// Makes an invite to the server, then shows it.
    /// The web's server dropdown (ChannelSidebar's ServerMenu), in its order:
    /// invite and what you manage, then rules, welcome and notifications,
    /// then the ID and leaving.
    fn server_header_items(&self, key: &str, server: &str, developer: Vec<Item>) -> Built {
        let Some((access, rules, welcome, onboarding)) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let sv = i.server(server)?;
            Some((i.access(server), sv.has_rules, sv.has_welcome_screen, sv.has_onboarding))
        }) else {
            return Built::of(Vec::new());
        };
        let mut manage = Vec::new();
        if access.has(P::CreateInvite) || access.channels.keys().any(|c| access.has_in(c, P::CreateInvite)) {
            manage.push(self.invite_item(key, server));
        }
        if crate::ui::server_settings::can_open(&access) {
            let (k, s) = (key.to_owned(), server.to_owned());
            manage.push(Item::act(
                "Server settings",
                "settings",
                run(move |this, window, cx| this.open_server_settings(&k, &s, window, cx)),
            ));
        }
        if access.has(P::ManageChannels) {
            let (k, s) = (key.to_owned(), server.to_owned());
            manage.push(Item::act(
                "Create channel",
                "plus",
                run(move |this, window, cx| {
                    let dialog = Dialog::CreateChannel {
                        key: k.clone(),
                        server: s.clone(),
                        parent: String::new(),
                        kind: pb::ChannelType::Text,
                    };
                    this.open_dialog(dialog, window, cx)
                }),
            ));
        }
        let mut about = Vec::new();
        if rules {
            let (k, s) = (key.to_owned(), server.to_owned());
            about.push(Item::act(
                "Rules",
                "scroll-text",
                run(move |this, window, cx| {
                    this.open_dialog(Dialog::Rules { key: k.clone(), server: s.clone() }, window, cx)
                }),
            ));
        }
        if welcome || onboarding {
            let (k, s) = (key.to_owned(), server.to_owned());
            about.push(Item::act(
                if onboarding { "Channels & roles" } else { "Welcome screen" },
                "party-popper",
                run(move |this, window, cx| {
                    if onboarding {
                        this.open_onboarding(&k, &s, cx)
                    } else {
                        this.open_dialog(Dialog::Welcome { key: k.clone(), server: s.clone() }, window, cx)
                    }
                }),
            ));
        }
        about.extend(self.notification_items(key, server, ""));
        {
            let menu = Menu::Server { key: key.to_owned(), server: server.to_owned() };
            about.push(Item::act(
                "Notification settings",
                "bell-ring",
                run(move |this, _, cx| {
                    this.menu = Some(menu.clone());
                    cx.notify();
                }),
            ));
        }
        about.extend(self.live_tile_menu_items(key, server));
        let mut danger = Vec::new();
        if !access.owner {
            let (k, s) = (key.to_owned(), server.to_owned());
            danger.push(
                Item::act(
                    "Leave server",
                    "door-open",
                    run(move |this, window, cx| {
                        this.open_dialog(Dialog::LeaveServer { key: k.clone(), server: s.clone() }, window, cx)
                    }),
                )
                .danger(),
            );
        }
        Built::of(vec![manage, about, developer, danger])
    }

    fn invite_item(&self, key: &str, server: &str) -> Item {
        let (k, s) = (key.to_owned(), server.to_owned());
        Item::act(
            "Invite people",
            "user-plus",
            run(move |this, window, cx| {
                let (core, k, s) = (this.core.clone(), k.clone(), s.clone());
                let server = s.clone();
                let rx = core.spawn({
                    let core = core.clone();
                    async move { core.create_invite(&k, &s).await }
                });
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(result) = rx.await else { return };
                    let _ = this.update_in(cx, |this, window, cx| match result {
                        Ok(link) => this.open_dialog(Dialog::Invite { link: Some(link), server }, window, cx),
                        Err(err) => {
                            this.toast("circle-alert", "Couldn't make an invite".into(), err.message, None, None, cx)
                        }
                    });
                })
                .detach();
            }),
        )
    }

    /// Mute (or unmute) a server or channel for a while, and, for a channel, what it notifies about.
    fn notification_items(&self, key: &str, server: &str, channel: &str) -> Vec<Item> {
        let own =
            self.core.shared.read(|s| s.instance(key).and_then(|i| i.notification_settings(server, channel).cloned()));
        let what = if channel.is_empty() { "server" } else { "channel" };
        let save = |patch: NotificationPatch| {
            let (k, s, c) = (key.to_owned(), server.to_owned(), channel.to_owned());
            run(move |this, _, cx| this.save_notifications(&k, &s, &c, patch.clone(), cx))
        };
        let mut out = Vec::new();
        if crate::core::notifications::is_muted(own.as_ref(), now_ms()) {
            let until = muted_label(own.as_ref());
            out.push(
                Item::act(
                    format!("Unmute {what}"),
                    "bell",
                    save(NotificationPatch { unmute: true, ..Default::default() }),
                )
                .hint(until.strip_prefix("Muted").unwrap_or("").trim().to_owned()),
            );
        } else {
            let items = MUTE_FOR
                .into_iter()
                .map(|(label, ms)| {
                    Item::act(
                        label,
                        "",
                        run({
                            let (k, s, c) = (key.to_owned(), server.to_owned(), channel.to_owned());
                            move |this, _, cx| {
                                let patch = NotificationPatch {
                                    mute_until: Some(ms.map(|ms| now_ms() + ms)),
                                    ..Default::default()
                                };
                                this.save_notifications(&k, &s, &c, patch, cx)
                            }
                        }),
                    )
                })
                .collect();
            out.push(Item::sub(format!("Mute {what}"), "bell-off", items));
        }
        if !channel.is_empty() {
            let level = own.as_ref().map(|n| n.level()).unwrap_or(Level::Unspecified);
            let choices = [
                (Level::Unspecified, "Use the server's", "Server's"),
                (Level::All, "All messages", "All"),
                (Level::Mentions, "Only @mentions", "@mentions"),
                (Level::Nothing, "Nothing", "Nothing"),
            ];
            let hint = choices.iter().find(|c| c.0 == level).map(|c| c.2).unwrap_or("Server's");
            let items = choices
                .iter()
                .map(|(l, label, _)| {
                    Item::check(
                        *label,
                        *l == level,
                        true,
                        save(NotificationPatch { level: Some(*l), ..Default::default() }),
                    )
                })
                .collect();
            out.push(Item::sub("Notifications", "sliders-horizontal", items).hint(hint));
        }
        out
    }

    fn edit_channel_item(&self, label: &str, key: &str, server: &str, channel: &str, permissions: bool) -> Item {
        let (k, s, c) = (key.to_owned(), server.to_owned(), channel.to_owned());
        Item::act(
            label,
            if permissions { "key-round" } else { "settings" },
            run(move |this, window, cx| {
                this.open_server_settings(&k, &s, window, cx);
                if let Some(view) = &this.server_settings {
                    let c = c.clone();
                    view.update(cx, |view, cx| view.edit_channel(c, permissions, cx));
                }
            }),
        )
    }

    fn delete_channel_item(&self, label: &str, key: &str, server: &str, channel: &str, name: &str, body: &str) -> Item {
        let (k, s, c, shown) = (key.to_owned(), server.to_owned(), channel.to_owned(), name.to_owned());
        Item::act(
            label,
            "trash",
            run(move |this, _, cx| {
                let (core, k, s, c, shown) = (this.core.clone(), k.clone(), s.clone(), c.clone(), shown.clone());
                this.run(cx, async move { core.delete_channel(&k, &s, &c).await }, move |this, result, cx| {
                    match result {
                        Ok(()) => this.toast("trash", format!("Deleted {shown}"), String::new(), None, None, cx),
                        Err(err) => {
                            this.toast("circle-alert", "Couldn't delete that".into(), err.message, None, None, cx)
                        }
                    }
                    cx.notify();
                });
            }),
        )
        .danger()
        .confirm(format!("Delete {name}?"), body, label)
    }
}

/// Someone this instance knows of, from any server or conversation.
fn known_user(i: &InstanceState, user_id: &str) -> Option<pb::User> {
    i.members.values().flatten().filter_map(|m| m.user.as_ref()).find(|u| u.id == user_id).cloned()
}

/// A link to a place, on the instance's own address as invite links are, so
/// anyone with access can open it.
fn place_link(i: &InstanceState, path: &[&str]) -> String {
    let base = i.node.as_ref().map(|n| n.public_url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| i.url.clone());
    let mut link = base.trim_end_matches('/').to_owned();
    for part in std::iter::once(i.key.as_str()).chain(path.iter().copied()) {
        link.push('/');
        link.push_str(&urlencoding_lite(part));
    }
    link
}

/// Ids and keys are plain, but an instance key may hold a `:`.
fn urlencoding_lite(part: &str) -> String {
    part.chars()
        .map(
            |c| {
                if c.is_ascii_alphanumeric() || "-_.~".contains(c) {
                    c.to_string()
                } else {
                    format!("%{:02X}", c as u32)
                }
            },
        )
        .collect()
}

/// Whether you may make a copy of a channel, as the server checks: Manage
/// Channels where it goes, and to copy who may see it, Manage Roles there
/// and ranking above everyone it names. Never a secure or shared channel.
pub(crate) fn may_duplicate(i: &InstanceState, server: &str, c: &pb::Channel) -> bool {
    let access = i.access(server);
    let kind = pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text);
    if kind == pb::ChannelType::Secure || kind == pb::ChannelType::Category || c.shared.is_some() {
        return false;
    }
    let there = |p: P| if c.parent_id.is_empty() { access.has(p) } else { access.has_in(&c.parent_id, p) };
    if !there(P::ManageChannels) {
        return false;
    }
    if c.permission_overwrites.is_empty() {
        return true;
    }
    if !there(P::ManageRoles) {
        return false;
    }
    let me = i.me.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
    let roles = i.roles.get(server).map(Vec::as_slice).unwrap_or_default();
    c.permission_overwrites.iter().all(|o| {
        if o.target_id == server || o.target_id == me {
            return true;
        }
        if o.target == pb::OverwriteTarget::Member as i32 {
            let known = i
                .members
                .get(server)
                .is_some_and(|l| l.iter().any(|m| m.user.as_ref().is_some_and(|u| u.id == o.target_id)));
            return !known || crate::core::moderation::outranks(&access, &i.standing(server, &o.target_id));
        }
        roles.iter().find(|r| r.id == o.target_id).is_none_or(|r| access.above(r.position))
    })
}

impl FuwaApp {
    /// For the bells and the menus alike: saves a change to how something notifies you.
    pub(crate) fn save_notifications(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        patch: NotificationPatch,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let core = self.core.clone();
        let (key, server, channel) = (key.to_owned(), server.to_owned(), channel.to_owned());
        self.run(
            cx,
            async move { core.update_notifications(&key, &server, &channel, patch).await },
            |this, result, cx| {
                if let Err(err) = result {
                    this.toast("circle-alert", "Couldn't change that".into(), err.message, None, None, cx);
                }
                cx.notify();
            },
        );
        cx.notify();
    }
}
