//! Putting a server's channels in order, the desktop's side of
//! `web/src/lib/arrange.ts`: how they sit (the ones outside any category
//! first, then each category with its own), where a dragged one lands, and
//! the order the server's `ReorderChannels` takes.

use std::sync::Arc;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::store;
use crate::pb;
use crate::rpc;

/// How a server's channels sit. The sidebar shows exactly this.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub loose: Vec<String>,
    pub categories: Vec<(String, Vec<String>)>,
}

/// Where a dragged channel or category lands: before `before`, or at the end when it's `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drop {
    Channel { id: String, parent: String, before: Option<String> },
    Category { id: String, before: Option<String> },
}

fn is_category(c: &pb::Channel) -> bool {
    c.r#type == pb::ChannelType::Category as i32
}

pub fn layout_of(channels: &[pb::Channel]) -> Layout {
    let categories: Vec<&pb::Channel> = channels.iter().filter(|c| is_category(c)).collect();
    let known = |id: &str| categories.iter().any(|c| c.id == id);
    Layout {
        loose: channels.iter().filter(|c| !is_category(c) && !known(&c.parent_id)).map(|c| c.id.clone()).collect(),
        categories: categories
            .iter()
            .map(|cat| {
                let children = channels
                    .iter()
                    .filter(|c| !is_category(c) && c.parent_id == cat.id)
                    .map(|c| c.id.clone())
                    .collect();
                (cat.id.clone(), children)
            })
            .collect(),
    }
}

/// The layout as the server takes it: every channel once, in display order, each with its category.
pub fn placements(layout: &Layout) -> Vec<pb::ChannelPlacement> {
    let place = |id: &str, parent: &str| pb::ChannelPlacement { channel_id: id.into(), parent_id: parent.into() };
    let mut out: Vec<_> = layout.loose.iter().map(|id| place(id, "")).collect();
    for (cat, children) in &layout.categories {
        out.push(place(cat, ""));
        out.extend(children.iter().map(|id| place(id, cat)));
    }
    out
}

fn insert(list: &[String], id: &str, before: Option<&str>) -> Vec<String> {
    let mut rest: Vec<String> = list.iter().filter(|x| *x != id).cloned().collect();
    let at = before.and_then(|b| rest.iter().position(|x| x == b)).unwrap_or(rest.len());
    rest.insert(at, id.to_owned());
    rest
}

/// The layout after a drop. Dropping something where it already is gives back an equal layout.
pub fn moved(layout: &Layout, drop: &Drop) -> Layout {
    match drop {
        Drop::Category { id, before } => {
            if !layout.categories.iter().any(|(c, _)| c == id) {
                return layout.clone();
            }
            let ids: Vec<String> = layout.categories.iter().map(|(c, _)| c.clone()).collect();
            let order = insert(&ids, id, before.as_deref());
            Layout {
                loose: layout.loose.clone(),
                categories: order
                    .iter()
                    .filter_map(|id| layout.categories.iter().find(|(c, _)| c == id).cloned())
                    .collect(),
            }
        }
        Drop::Channel { id, parent, before } => {
            let without = |list: &[String]| list.iter().filter(|x| *x != id).cloned().collect::<Vec<_>>();
            let loose = without(&layout.loose);
            let categories: Vec<(String, Vec<String>)> =
                layout.categories.iter().map(|(c, children)| (c.clone(), without(children))).collect();
            if parent.is_empty() {
                return Layout { loose: insert(&loose, id, before.as_deref()), categories };
            }
            if !categories.iter().any(|(c, _)| c == parent) {
                return layout.clone();
            }
            Layout {
                loose,
                categories: categories
                    .into_iter()
                    .map(|(c, children)| {
                        let children = if &c == parent { insert(&children, id, before.as_deref()) } else { children };
                        (c, children)
                    })
                    .collect(),
            }
        }
    }
}

/// The channels as they'll be once the server takes a new order, for showing
/// it straight away. They keep the positions they had between them, handed
/// out in the new order, so the server's answer (and its events) usually
/// change nothing.
pub fn arranged(channels: &[pb::Channel], order: &[pb::ChannelPlacement]) -> Vec<pb::Channel> {
    let mut positions: Vec<i32> = channels.iter().map(|c| c.position).collect();
    positions.sort_unstable();
    let placed: Vec<pb::Channel> = order
        .iter()
        .enumerate()
        .filter_map(|(n, p)| {
            let c = channels.iter().find(|c| c.id == p.channel_id)?;
            Some(pb::Channel {
                position: positions.get(n).copied().unwrap_or(n as i32),
                parent_id: p.parent_id.clone(),
                ..c.clone()
            })
        })
        .collect();
    if placed.len() == channels.len() { placed } else { channels.to_vec() }
}

impl Core {
    /// Puts a server's channels in a new order: shown straight away, and put
    /// back as they were if the instance says no.
    pub async fn reorder_channels(self: &Arc<Self>, key: &str, server_id: &str, layout: Layout) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(tonic::Code::NotFound, "That instance isn't here."))?;
        let order = placements(&layout);
        let before = self.shared.read(|s| s.instance(key).and_then(|i| i.channels.get(server_id).cloned()));
        if let Some(before) = &before {
            let now = arranged(before, &order);
            self.shared.instance(key, |i| store::set_channels(i, server_id, now));
        }
        let request = pb::ReorderChannelsRequest { server_id: server_id.into(), channels: order };
        match rpc!(api.channels(), reorder_channels(request)).await {
            Ok(res) => {
                self.shared.instance(key, |i| store::set_channels(i, server_id, res.channels));
                Ok(())
            }
            Err(err) => {
                if let Some(before) = before {
                    self.shared.instance(key, |i| store::set_channels(i, server_id, before));
                }
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: &str, parent: &str, category: bool, position: i32) -> pb::Channel {
        pb::Channel {
            id: id.into(),
            parent_id: parent.into(),
            position,
            r#type: if category { pb::ChannelType::Category } else { pb::ChannelType::Text } as i32,
            ..Default::default()
        }
    }

    fn sample() -> Vec<pb::Channel> {
        vec![
            channel("general", "", false, 0),
            channel("games", "", true, 1),
            channel("minecraft", "games", false, 2),
            channel("chess", "games", false, 3),
            channel("art", "", true, 4),
            channel("doodles", "art", false, 5),
        ]
    }

    #[test]
    fn channels_move_between_categories_and_categories_between_each_other() {
        let layout = layout_of(&sample());
        assert_eq!(layout.loose, ["general"]);
        assert_eq!(layout.categories[0], ("games".into(), vec!["minecraft".into(), "chess".into()]));

        let drop = Drop::Channel { id: "chess".into(), parent: "art".into(), before: Some("doodles".into()) };
        let next = moved(&layout, &drop);
        assert_eq!(next.categories[0].1, ["minecraft"]);
        assert_eq!(next.categories[1].1, ["chess", "doodles"]);

        let out = moved(&layout, &Drop::Channel { id: "minecraft".into(), parent: String::new(), before: None });
        assert_eq!(out.loose, ["general", "minecraft"]);

        let swapped = moved(&layout, &Drop::Category { id: "art".into(), before: Some("games".into()) });
        let ids: Vec<&str> = swapped.categories.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(ids, ["art", "games"]);
        assert_eq!(swapped.categories[1].1, ["minecraft", "chess"], "a category takes its channels along");

        let same = moved(&layout, &Drop::Channel { id: "chess".into(), parent: "games".into(), before: None });
        assert_eq!(same, layout, "dropping where it is changes nothing");
        let nowhere = moved(&layout, &Drop::Channel { id: "chess".into(), parent: "gone".into(), before: None });
        assert_eq!(nowhere, layout);
    }

    #[test]
    fn the_new_order_keeps_the_positions_it_had() {
        let channels = sample();
        let next = moved(&layout_of(&channels), &Drop::Category { id: "art".into(), before: Some("games".into()) });
        let order = placements(&next);
        let ids: Vec<&str> = order.iter().map(|p| p.channel_id.as_str()).collect();
        assert_eq!(ids, ["general", "art", "doodles", "games", "minecraft", "chess"]);
        let shown = arranged(&channels, &order);
        let positions: Vec<(&str, i32)> = shown.iter().map(|c| (c.id.as_str(), c.position)).collect();
        assert_eq!(
            positions,
            [("general", 0), ("art", 1), ("doodles", 2), ("games", 3), ("minecraft", 4), ("chess", 5)]
        );
        assert_eq!(shown[2].parent_id, "art");
    }
}
