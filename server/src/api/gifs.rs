use std::time::Instant;

use tonic::{Request, Response, Status};

use super::{Api, respond};
use crate::error::{Error, Result};
use crate::gifs::{self, Ask, store};
use crate::pb::{self, gif_service_server::GifService};

/// Results a page holds unless asked otherwise.
const PAGE: u32 = 24;

#[tonic::async_trait]
impl GifService for Api {
    async fn get_gif_settings(
        &self,
        request: Request<pb::GetGifSettingsRequest>,
    ) -> Result<Response<pb::GetGifSettingsResponse>, Status> {
        respond(
            async {
                self.account(request.metadata()).await?;
                let setup = &self.app.settings().gifs;
                Ok(pb::GetGifSettingsResponse {
                    enabled: setup.usable(),
                    provider: if setup.usable() { setup.provider.to_pb() as i32 } else { 0 },
                })
            }
            .await,
        )
    }

    async fn search_gifs(
        &self,
        request: Request<pb::SearchGifsRequest>,
    ) -> Result<Response<pb::SearchGifsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let setup = searching(&self.app)?;
                let query = req.query.split_whitespace().collect::<Vec<_>>().join(" ");
                if query.chars().count() > gifs::MAX_QUERY {
                    return Err(Error::invalid(format!("search for up to {} characters", gifs::MAX_QUERY)));
                }
                let cursor = match req.cursor.trim() {
                    "" => 0,
                    c => c.parse::<u32>().map_err(|_| Error::invalid("that page doesn't exist"))?,
                };
                if cursor > 5000 {
                    return Err(Error::invalid("that page doesn't exist"));
                }
                let limit = if req.limit <= 0 { PAGE } else { (req.limit as u32).min(50) };
                gifs::take_turn(&account.id, setup.searches_per_minute)?;
                let page = gifs::ask(&self.app, &setup, Ask { query: &query, cursor, limit }).await?;
                Ok(pb::SearchGifsResponse {
                    results: page.found.iter().map(|found| gifs::result(&self.app, setup.provider, found)).collect(),
                    next_cursor: page.next.unwrap_or_default(),
                })
            }
            .await,
        )
    }

    async fn list_gif_categories(
        &self,
        request: Request<pb::ListGifCategoriesRequest>,
    ) -> Result<Response<pb::ListGifCategoriesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let setup = searching(&self.app)?;
                gifs::take_turn(&account.id, setup.searches_per_minute)?;
                Ok(pb::ListGifCategoriesResponse { categories: gifs::categories(&self.app, &setup).await? })
            }
            .await,
        )
    }

    async fn prepare_gif(
        &self,
        request: Request<pb::PrepareGifRequest>,
    ) -> Result<Response<pb::PrepareGifResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let gif = match request.into_inner().from {
                    Some(pb::prepare_gif_request::From::ResultId(id)) => {
                        let setup = searching(&self.app)?;
                        let token = gifs::open_result(&self.app, &id)?;
                        gifs::take_turn(&account.id, setup.searches_per_minute)?;
                        store::store_found(&self.app, &setup, &token).await?
                    }
                    Some(pb::prepare_gif_request::From::UploadUrl(url)) => {
                        store::from_upload(&self.app, &account.id, &url).await?.sealed(&self.app)
                    }
                    None => return Err(Error::invalid("pick a GIF")),
                };
                Ok(pb::PrepareGifResponse { gif: Some(gif) })
            }
            .await,
        )
    }

    async fn list_saved_gifs(
        &self,
        request: Request<pb::ListSavedGifsRequest>,
    ) -> Result<Response<pb::ListSavedGifsResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let saved = store::saved(self.app.node()?, &account.id).await?;
                Ok(pb::ListSavedGifsResponse {
                    gifs: saved
                        .iter()
                        .map(|(file, at, uploaded)| store::saved_pb(&self.app, file, *at, *uploaded))
                        .collect(),
                })
            }
            .await,
        )
    }

    async fn save_gif(&self, request: Request<pb::SaveGifRequest>) -> Result<Response<pb::SaveGifResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let url = request.into_inner().url;
                let media_id = crate::media::id_in_url(&url).ok_or_else(|| Error::invalid("that isn't a GIF here"))?;
                let node = self.app.node()?;
                let (file, uploaded) = match store::by_media(node, &media_id).await? {
                    Some(file) => {
                        let mine = node.media(&media_id).await?.is_some_and(|row| row.account_id == account.id);
                        (file, mine)
                    }
                    // One of your own uploads, not sent yet.
                    None => (store::from_upload(&self.app, &account.id, &url).await?, true),
                };
                let at = store::save(node, &account.id, &file.media_id).await?;
                Ok(pb::SaveGifResponse { gif: Some(store::saved_pb(&self.app, &file, at, uploaded)) })
            }
            .await,
        )
    }

    async fn delete_saved_gif(
        &self,
        request: Request<pb::DeleteSavedGifRequest>,
    ) -> Result<Response<pb::DeleteSavedGifResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let url = request.into_inner().url;
                let media_id = crate::media::id_in_url(&url).ok_or_else(|| Error::invalid("that isn't a GIF here"))?;
                // The file stays: messages may show it.
                store::unsave(self.app.node()?, &account.id, &media_id).await?;
                Ok(pb::DeleteSavedGifResponse {})
            }
            .await,
        )
    }

    async fn test_gif_provider(
        &self,
        request: Request<pb::TestGifProviderRequest>,
    ) -> Result<Response<pb::TestGifProviderResponse>, Status> {
        respond(
            async {
                self.require_instance_admin(request.metadata()).await?;
                let given = request.into_inner().settings.unwrap_or_default();
                let setup = gifs::Setup::from_pb(&given, &self.app.settings().gifs)?;
                if !setup.usable() {
                    return Err(Error::invalid("pick a provider and give its key first"));
                }
                let started = Instant::now();
                let answer = gifs::fetch_page(&self.app, &setup, Ask { query: "", cursor: 0, limit: 6 }).await;
                let elapsed_ms = started.elapsed().as_millis().min(i32::MAX as u128) as i32;
                Ok(match answer {
                    Ok(page) => pb::TestGifProviderResponse {
                        ok: true,
                        error: String::new(),
                        elapsed_ms,
                        results: page.found.iter().map(|f| gifs::result(&self.app, setup.provider, f)).collect(),
                    },
                    Err(failure) => pb::TestGifProviderResponse {
                        ok: false,
                        error: failure.to_string(),
                        elapsed_ms,
                        results: vec![],
                    },
                })
            }
            .await,
        )
    }
}

/// The GIF settings, when search is on.
fn searching(app: &crate::app::App) -> Result<gifs::Setup> {
    let setup = app.settings().gifs.clone();
    if !setup.usable() {
        return Err(Error::FailedPrecondition("GIF search is off on this instance".into()));
    }
    Ok(setup)
}
