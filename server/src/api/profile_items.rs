//! Profile effects and avatar decorations the instance or a server offers
//! (`ProfileItemService`, docs/profile-items.md). The instance's live in
//! node.db and are answered where accounts are; a server's live in its file,
//! where every change goes out to members as `ProfileItemsUpdated`.

use tonic::{Request, Response, Status};

use super::{Api, PictureOwner, respond};
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::node::Account;
use crate::pb::{self, Permission, profile_item_service_server::ProfileItemService};
use crate::profile_items as items;
use crate::servers::{self as store, Audit, Payload, UsageChange};

/// Every item the server has now, as the event members get.
async fn changed(conn: &turso::Connection, server_id: &str, events: &mut Vec<Payload>) -> Result<()> {
    let items = items::list(conn, server_id).await?;
    events.push(Payload::ProfileItemsUpdated(pb::ProfileItemsUpdated { items }));
    Ok(())
}

impl Api {
    /// A new item checked, with its pictures: uploads of the caller's for
    /// `server_id` (or for the instance, without one).
    async fn new_item(
        &self,
        account: &Account,
        new: Option<pb::NewProfileItem>,
        server_id: Option<&str>,
    ) -> Result<(pb::ProfileItem, Vec<pb::Media>)> {
        let new = new.ok_or_else(|| Error::invalid("item is needed"))?;
        let mut item = items::checked_new(&new, &new_id())?;
        item.server_id = server_id.unwrap_or_default().to_string();
        item.creator_id = account.id.clone();
        item.created_at = Some(timestamp(now_ms()));
        if item.kind == pb::ProfileItemKind::Effect as i32 {
            let (pictures, uploads) = self.effect_pictures(account, &[], &new.picture_urls, server_id).await?;
            item.size = pictures.iter().map(|p| p.size).sum();
            item.pictures = pictures;
            return Ok((item, uploads));
        }
        let upload = self
            .check_upload(account, pb::MediaPurpose::Decoration, new.picture_url.trim(), server_id)
            .await?
            .ok_or_else(|| Error::invalid("upload the decoration's picture to this fuwa server first"))?;
        item.picture_url = upload.url.clone();
        item.animated = upload.content_type == "image/gif";
        item.size = upload.size;
        Ok((item, vec![upload]))
    }

    /// An effect's pictures for `urls`, in order: the ones it has already
    /// (`had`) as they are, and new ones checked as uploads of the caller's,
    /// which are handed back too, to keep once they're saved.
    async fn effect_pictures(
        &self,
        account: &Account,
        had: &[pb::ProfileItemPicture],
        urls: &[String],
        server_id: Option<&str>,
    ) -> Result<(Vec<pb::ProfileItemPicture>, Vec<pb::Media>)> {
        if urls.len() > items::MAX_PICTURES {
            return Err(Error::invalid("an effect has up to 8 pictures"));
        }
        let mut pictures: Vec<pb::ProfileItemPicture> = Vec::with_capacity(urls.len());
        let mut uploads = Vec::new();
        for url in urls {
            let url = url.trim();
            if pictures.iter().any(|p| p.url == url) {
                return Err(Error::invalid("an effect has each picture once"));
            }
            if let Some(kept) = had.iter().find(|p| p.url == url) {
                pictures.push(kept.clone());
                continue;
            }
            let upload = self
                .check_upload(account, pb::MediaPurpose::Decoration, url, server_id)
                .await?
                .ok_or_else(|| Error::invalid("upload the effect's pictures to this fuwa server first"))?;
            pictures.push(pb::ProfileItemPicture {
                url: upload.url.clone(),
                animated: upload.content_type == "image/gif",
                size: upload.size,
            });
            uploads.push(upload);
        }
        Ok((pictures, uploads))
    }

    /// Marks an item's new pictures used.
    async fn keep_pictures(&self, uploads: &[pb::Media], server_id: Option<&str>) {
        for upload in uploads {
            self.keep_picture(Some(&upload.id), server_id).await;
        }
    }

    /// Deletes the pictures `before` had and `after` doesn't (all of them
    /// when it's gone).
    async fn drop_pictures(&self, before: &pb::ProfileItem, after: Option<&pb::ProfileItem>, owner: PictureOwner<'_>) {
        for url in items::picture_urls(before) {
            if !after.is_some_and(|after| items::picture_urls(after).any(|u| u == url)) {
                self.drop_picture(url, "", owner).await;
            }
        }
    }

    /// Whether a server has room for `more` bytes of files.
    async fn room_for(&self, sdb: &crate::servers::ServerDb, more: i64) -> Result<()> {
        if more <= 0 {
            return Ok(());
        }
        let limits = sdb.limits(&self.app.settings().limits).await?;
        if let Some(limit) = limits.attachment_bytes
            && sdb.usage().await?.attachment_bytes + more > limit
        {
            return Err(Error::ResourceExhausted("this server is out of room for files".into()));
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl ProfileItemService for Api {
    async fn list_instance_profile_items(
        &self,
        request: Request<pb::ListInstanceProfileItemsRequest>,
    ) -> Result<Response<pb::ListInstanceProfileItemsResponse>, Status> {
        respond(
            async {
                self.account(request.metadata()).await?;
                Ok(pb::ListInstanceProfileItemsResponse { items: self.app.node()?.profile_items().await? })
            }
            .await,
        )
    }

    async fn create_instance_profile_item(
        &self,
        request: Request<pb::CreateInstanceProfileItemRequest>,
    ) -> Result<Response<pb::CreateInstanceProfileItemResponse>, Status> {
        respond(
            async {
                let viewer = self.require_instance_admin(request.metadata()).await?;
                let account = viewer.account()?;
                let (item, uploads) = self.new_item(account, request.into_inner().item, None).await?;
                self.app.node()?.add_profile_item(&item).await?;
                self.keep_pictures(&uploads, None).await;
                tracing::info!("added a profile item for the instance");
                Ok(pb::CreateInstanceProfileItemResponse { item: Some(item) })
            }
            .await,
        )
    }

    async fn update_instance_profile_item(
        &self,
        request: Request<pb::UpdateInstanceProfileItemRequest>,
    ) -> Result<Response<pb::UpdateInstanceProfileItemResponse>, Status> {
        respond(
            async {
                let viewer = self.require_instance_admin(request.metadata()).await?;
                let account = viewer.account()?;
                let req = request.into_inner();
                let change = req.change.unwrap_or_default();
                let node = self.app.node()?;
                let before = node.profile_item(&req.item_id).await?.ok_or(Error::NotFound("profile item"))?;
                let mut item = before.clone();
                let mut uploads = Vec::new();
                if let Some(pictures) = &change.pictures {
                    let (pictures, new) = self.effect_pictures(account, &before.pictures, &pictures.urls, None).await?;
                    item.size = pictures.iter().map(|p| p.size).sum();
                    item.pictures = pictures;
                    uploads = new;
                }
                if items::apply(&mut item, &change)? || item.pictures != before.pictures {
                    node.save_profile_item(&item).await?;
                    self.keep_pictures(&uploads, None).await;
                    self.drop_pictures(&before, Some(&item), PictureOwner::Instance).await;
                }
                Ok(pb::UpdateInstanceProfileItemResponse { item: Some(item) })
            }
            .await,
        )
    }

    async fn delete_instance_profile_item(
        &self,
        request: Request<pb::DeleteInstanceProfileItemRequest>,
    ) -> Result<Response<pb::DeleteInstanceProfileItemResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                let req = request.into_inner();
                let item =
                    self.app.node()?.delete_profile_item(&req.item_id).await?.ok_or(Error::NotFound("profile item"))?;
                // Copies of who wears it in servers' files keep the id, which
                // apps no longer find, so they draw nothing.
                self.drop_pictures(&item, None, PictureOwner::Instance).await;
                tracing::info!("deleted one of the instance's profile items");
                Ok(pb::DeleteInstanceProfileItemResponse {})
            }
            .await,
        )
    }

    async fn list_server_profile_items(
        &self,
        request: Request<pb::ListServerProfileItemsRequest>,
    ) -> Result<Response<pb::ListServerProfileItemsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.membership(&account, &request.get_ref().server_id).await?.sdb;
                Ok(pb::ListServerProfileItemsResponse { items: items::list(&*sdb.read()?, &sdb.id).await? })
            }
            .await,
        )
    }

    async fn create_server_profile_item(
        &self,
        request: Request<pb::CreateServerProfileItemRequest>,
    ) -> Result<Response<pb::CreateServerProfileItemResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let (item, uploads) = self.new_item(&account, req.item, Some(&sdb.id)).await?;
                self.room_for(&sdb, item.size).await?;
                let item = sdb
                    .write(&account.id, async |conn, events| {
                        items::insert(conn, &item).await?;
                        if items::files(&item) > 0 {
                            let usage = UsageChange {
                                attachments: items::files(&item),
                                attachment_bytes: item.size,
                                ..Default::default()
                            };
                            store::add_usage(conn, usage).await?;
                        }
                        let entry =
                            Audit::new(pb::AuditAction::ProfileItemCreate, &item.id).change("name", "", &item.name);
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(item.clone())
                    })
                    .await?;
                self.keep_pictures(&uploads, Some(&sdb.id)).await;
                Ok(pb::CreateServerProfileItemResponse { item: Some(item) })
            }
            .await,
        )
    }

    async fn update_server_profile_item(
        &self,
        request: Request<pb::UpdateServerProfileItemRequest>,
    ) -> Result<Response<pb::UpdateServerProfileItemResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let change = req.change.unwrap_or_default();
                // New pictures are checked before the write, against the item as it is now.
                let (pictures, uploads) = match &change.pictures {
                    Some(pictures) => {
                        let had = items::get(&*sdb.read()?, &sdb.id, &req.item_id)
                            .await?
                            .ok_or(Error::NotFound("profile item"))?
                            .pictures;
                        let (pictures, uploads) =
                            self.effect_pictures(&account, &had, &pictures.urls, Some(&sdb.id)).await?;
                        self.room_for(&sdb, uploads.iter().map(|u| u.size).sum()).await?;
                        (Some(pictures), uploads)
                    }
                    None => (None, Vec::new()),
                };
                let (before, item) = sdb
                    .write(&account.id, async |conn, events| {
                        let mut item =
                            items::get(conn, &sdb.id, &req.item_id).await?.ok_or(Error::NotFound("profile item"))?;
                        let before = item.clone();
                        if let Some(pictures) = &pictures {
                            item.pictures = pictures.clone();
                            item.size = pictures.iter().map(|p| p.size).sum();
                        }
                        if !items::apply(&mut item, &change)? && item.pictures == before.pictures {
                            return Ok((before, item));
                        }
                        items::save(conn, &item).await?;
                        let files = items::files(&item) - items::files(&before);
                        if files != 0 || item.size != before.size {
                            let usage = UsageChange {
                                attachments: files,
                                attachment_bytes: item.size - before.size,
                                ..Default::default()
                            };
                            store::add_usage(conn, usage).await?;
                        }
                        let mut entry = Audit::new(pb::AuditAction::ProfileItemUpdate, &item.id)
                            .change("name", &before.name, &item.name)
                            .change("description", &before.description, &item.description);
                        if before.effect != item.effect {
                            entry = entry.change("effect", "", "changed");
                        }
                        if before.pictures != item.pictures {
                            let count = |item: &pb::ProfileItem| item.pictures.len().to_string();
                            entry = entry.change("pictures", count(&before), count(&item));
                        }
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok((before, item))
                    })
                    .await?;
                // Uploads that didn't make it in (a clash took them out) are left to the sweeps.
                let kept: Vec<_> =
                    uploads.into_iter().filter(|u| item.pictures.iter().any(|p| p.url == u.url)).collect();
                self.keep_pictures(&kept, Some(&sdb.id)).await;
                self.drop_pictures(&before, Some(&item), PictureOwner::Server(&sdb.id)).await;
                Ok(pb::UpdateServerProfileItemResponse { item: Some(item) })
            }
            .await,
        )
    }

    async fn delete_server_profile_item(
        &self,
        request: Request<pb::DeleteServerProfileItemRequest>,
    ) -> Result<Response<pb::DeleteServerProfileItemResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let sdb = self.with(&account, &req.server_id, Permission::ManageServer).await?.sdb;
                let item = sdb
                    .write(&account.id, async |conn, events| {
                        let item =
                            items::get(conn, &sdb.id, &req.item_id).await?.ok_or(Error::NotFound("profile item"))?;
                        items::delete(conn, &item.id).await?;
                        // Off everyone wearing it; apps find it gone from the list.
                        conn.execute(
                            "UPDATE members SET profile_effect = '' WHERE profile_effect = ?1",
                            [item.id.as_str()],
                        )
                        .await?;
                        conn.execute(
                            "UPDATE members SET profile_decoration = '' WHERE profile_decoration = ?1",
                            [item.id.as_str()],
                        )
                        .await?;
                        if items::files(&item) > 0 {
                            let usage = UsageChange {
                                attachments: -items::files(&item),
                                attachment_bytes: -item.size,
                                ..Default::default()
                            };
                            store::add_usage(conn, usage).await?;
                        }
                        let entry =
                            Audit::new(pb::AuditAction::ProfileItemDelete, &item.id).change("name", &item.name, "");
                        store::audit(conn, &account.id, entry).await?;
                        super::economy::unlist(conn, &sdb.id, pb::ShopListingKind::ProfileItem, &item.id, events)
                            .await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(item)
                    })
                    .await?;
                self.drop_pictures(&item, None, PictureOwner::Server(&sdb.id)).await;
                Ok(pb::DeleteServerProfileItemResponse {})
            }
            .await,
        )
    }
}
