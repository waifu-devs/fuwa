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
    /// A new item checked, with its decoration's picture: an upload of the
    /// caller's for `server_id` (or for the instance, without one).
    async fn new_item(
        &self,
        account: &Account,
        new: Option<pb::NewProfileItem>,
        server_id: Option<&str>,
    ) -> Result<(pb::ProfileItem, Option<pb::Media>)> {
        let new = new.ok_or_else(|| Error::invalid("item is needed"))?;
        let mut item = items::checked_new(&new, &new_id())?;
        item.server_id = server_id.unwrap_or_default().to_string();
        item.creator_id = account.id.clone();
        item.created_at = Some(timestamp(now_ms()));
        if item.kind != pb::ProfileItemKind::Decoration as i32 {
            return Ok((item, None));
        }
        let upload = self
            .check_upload(account, pb::MediaPurpose::Decoration, new.picture_url.trim(), server_id)
            .await?
            .ok_or_else(|| Error::invalid("upload the decoration's picture to this fuwa server first"))?;
        item.picture_url = upload.url.clone();
        item.animated = upload.content_type == "image/gif";
        item.size = upload.size;
        Ok((item, Some(upload)))
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
                let (item, upload) = self.new_item(account, request.into_inner().item, None).await?;
                self.app.node()?.add_profile_item(&item).await?;
                self.keep_picture(upload.as_ref().map(|u| u.id.as_str()), None).await;
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
                self.require_instance_admin(request.metadata()).await?;
                let req = request.into_inner();
                let node = self.app.node()?;
                let mut item = node.profile_item(&req.item_id).await?.ok_or(Error::NotFound("profile item"))?;
                if items::apply(&mut item, &req.change.unwrap_or_default())? {
                    node.save_profile_item(&item).await?;
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
                let owner = PictureOwner::Account(&item.creator_id, pb::MediaPurpose::Decoration);
                self.drop_picture(&item.picture_url, "", owner).await;
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
                let (item, upload) = self.new_item(&account, req.item, Some(&sdb.id)).await?;
                if item.size > 0 {
                    let limits = sdb.limits(&self.app.settings().limits).await?;
                    if let Some(limit) = limits.attachment_bytes
                        && sdb.usage().await?.attachment_bytes + item.size > limit
                    {
                        return Err(Error::ResourceExhausted("this server is out of room for files".into()));
                    }
                }
                let item = sdb
                    .write(&account.id, async |conn, events| {
                        items::insert(conn, &item).await?;
                        if item.size > 0 {
                            let usage =
                                UsageChange { attachments: 1, attachment_bytes: item.size, ..Default::default() };
                            store::add_usage(conn, usage).await?;
                        }
                        let entry =
                            Audit::new(pb::AuditAction::ProfileItemCreate, &item.id).change("name", "", &item.name);
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(item.clone())
                    })
                    .await?;
                self.keep_picture(upload.as_ref().map(|u| u.id.as_str()), Some(&sdb.id)).await;
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
                let item = sdb
                    .write(&account.id, async |conn, events| {
                        let mut item =
                            items::get(conn, &sdb.id, &req.item_id).await?.ok_or(Error::NotFound("profile item"))?;
                        let before = item.clone();
                        if !items::apply(&mut item, &change)? {
                            return Ok(item);
                        }
                        items::save(conn, &item).await?;
                        let mut entry = Audit::new(pb::AuditAction::ProfileItemUpdate, &item.id)
                            .change("name", &before.name, &item.name)
                            .change("description", &before.description, &item.description);
                        if before.effect != item.effect {
                            entry = entry.change("effect", "", "changed");
                        }
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(item)
                    })
                    .await?;
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
                        if item.size > 0 {
                            let usage =
                                UsageChange { attachments: -1, attachment_bytes: -item.size, ..Default::default() };
                            store::add_usage(conn, usage).await?;
                        }
                        let entry =
                            Audit::new(pb::AuditAction::ProfileItemDelete, &item.id).change("name", &item.name, "");
                        store::audit(conn, &account.id, entry).await?;
                        changed(conn, &sdb.id, events).await?;
                        Ok(item)
                    })
                    .await?;
                self.drop_picture(&item.picture_url, "", PictureOwner::Server(&sdb.id)).await;
                Ok(pb::DeleteServerProfileItemResponse {})
            }
            .await,
        )
    }
}
