//! Where identity providers send people back: plain HTTP on the instance's
//! public URL. `/sso/instance/...` is the instance's own sign-in (served
//! where node.db is), `/sso/servers/<server id>/...` one server's (served
//! where that server is kept; gateways pass these on to its shard). Each has
//!   GET  .../oidc            the OpenID Connect redirect URI
//!   POST .../saml            the SAML Assertion Consumer Service
//!   GET  .../saml/metadata   this side's SAML metadata (and entity ID)
//! A good answer is saved with a one-time code and the browser goes on to
//! `<public URL>/auth/sso/done#...`; a refused one goes there with `error`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, extract::DefaultBodyLimit};
use http::{HeaderValue, StatusCode, header};
use serde::Deserialize;

use super::{Answer, Endpoints, Provider, Scope, SignIn};
use crate::app::App;
use crate::error::{Error, MISROUTED, Result};

/// SAML responses with a few certificates and attributes fit easily.
const MAX_BODY: usize = 256 * 1024;

#[derive(Deserialize, Default)]
#[serde(default)]
struct OidcQuery {
    state: String,
    code: String,
    error: String,
    error_description: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
#[allow(non_snake_case)]
struct SamlForm {
    SAMLResponse: String,
    RelayState: String,
}

/// The instance's own sign-in.
pub fn instance_routes(app: Arc<App>) -> Router {
    let (a, b, c) = (app.clone(), app.clone(), app);
    Router::new()
        .route(
            "/sso/instance/oidc",
            get(move |Query(q): Query<OidcQuery>| {
                let app = a.clone();
                async move { come_back(&app, Scope::Instance, q.state.clone(), oidc_answer(q)).await }
            }),
        )
        .route(
            "/sso/instance/saml",
            post(move |Form(f): Form<SamlForm>| {
                let app = b.clone();
                async move {
                    come_back(&app, Scope::Instance, f.RelayState, Answer::Saml { response: f.SAMLResponse }).await
                }
            }),
        )
        .route(
            "/sso/instance/saml/metadata",
            get(move || {
                let app = c.clone();
                async move { metadata(&Endpoints::new(&app.settings().public_url, &Scope::Instance)) }
            }),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

/// Each server's sign-in.
pub fn server_routes(app: Arc<App>) -> Router {
    let (a, b, c) = (app.clone(), app.clone(), app);
    Router::new()
        .route(
            "/sso/servers/{server_id}/oidc",
            get(move |Path(server_id): Path<String>, Query(q): Query<OidcQuery>| {
                let app = a.clone();
                async move { come_back(&app, Scope::Server(server_id), q.state.clone(), oidc_answer(q)).await }
            }),
        )
        .route(
            "/sso/servers/{server_id}/saml",
            post(move |Path(server_id): Path<String>, Form(f): Form<SamlForm>| {
                let app = b.clone();
                async move {
                    come_back(&app, Scope::Server(server_id), f.RelayState, Answer::Saml { response: f.SAMLResponse })
                        .await
                }
            }),
        )
        .route(
            "/sso/servers/{server_id}/saml/metadata",
            get(move |Path(server_id): Path<String>| {
                let app = c.clone();
                async move {
                    match app.servers.get(&server_id).await {
                        Ok(_) => metadata(&Endpoints::new(&app.settings().public_url, &Scope::Server(server_id))),
                        Err(Error::Misrouted) => misrouted(),
                        Err(err) => refused_page(err),
                    }
                }
            }),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

fn oidc_answer(q: OidcQuery) -> Answer {
    let error = if q.error.is_empty() {
        String::new()
    } else if q.error_description.is_empty() {
        q.error
    } else {
        format!("{}: {}", q.error, q.error_description)
    };
    Answer::Oidc { code: q.code, error: error.chars().take(300).collect() }
}

fn metadata(endpoints: &Endpoints) -> Response {
    let mut response = super::saml::metadata(&endpoints.entity_id, &endpoints.acs_url).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/samlmetadata+xml"));
    response
}

/// Takes a provider's answer for a sign-in, then sends the browser on.
async fn come_back(app: &Arc<App>, scope: Scope, state: String, answer: Answer) -> Response {
    let public_url = app.settings().public_url.clone();
    let endpoints = Endpoints::new(&public_url, &scope);
    let state = state.trim().to_string();
    let found = match load(app, &scope, &state).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            return refused_page(Error::FailedPrecondition("this sign-in ran out; start again from the app".into()));
        }
        Err(Error::Misrouted) => return misrouted(),
        Err(err) => return refused_page(err),
    };
    let (sign_in, provider) = found;
    // The answer rides in the fragment: browsers never send it to a server,
    // so the one-time code stays out of every access log on the way.
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Scope::Server(id) = &scope {
        pairs.push(("server", id.clone()));
    }
    pairs.push(("state", state.clone()));
    let result: Result<String> = async {
        let identity = super::identify(&provider, &endpoints, &sign_in, answer).await?;
        let code = crate::auth::new_token();
        if !answered(app, &scope, &sign_in, &crate::auth::hash_token(&code), &identity).await? {
            return Err(Error::FailedPrecondition("this sign-in was already used; start again".into()));
        }
        Ok(code)
    }
    .await;
    match result {
        Ok(code) => {
            pairs.push(("code", code));
        }
        Err(Error::Misrouted) => return misrouted(),
        Err(err) => {
            tracing::info!(error = %err, "a single sign-on answer was refused");
            pairs.push(("error", public_message(&err)));
        }
    }
    let mut done = reqwest::Url::parse(&format!("{}{}", endpoints.public_url, super::DONE))
        .unwrap_or_else(|_| reqwest::Url::parse("http://localhost/auth/sso/done").expect("a valid URL"));
    done.set_fragment(Some(&url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()));
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(location) = HeaderValue::from_str(done.as_str()) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

async fn load(app: &Arc<App>, scope: &Scope, state: &str) -> Result<Option<(SignIn, Provider)>> {
    match scope {
        Scope::Instance => {
            // Nothing was kept when it started: the state itself says what it is.
            let settings = app.settings();
            let provider = settings.sso_provider.clone();
            let sign_in = super::ticket::read(
                app.picture_key(),
                &provider,
                state,
                crate::id::now_ms(),
                &settings.public_url,
                &settings.allowed_origins,
            )?;
            Ok(Some((sign_in, provider)))
        }
        Scope::Server(id) => {
            let sdb = app.servers.get(id).await?;
            let conn = sdb.read()?;
            let Some(sign_in) = super::load(&conn, state).await? else { return Ok(None) };
            let provider = crate::servers::load_sso(&conn).await?.provider;
            Ok(Some((sign_in, provider)))
        }
    }
}

async fn answered(
    app: &Arc<App>,
    scope: &Scope,
    sign_in: &SignIn,
    code_hash: &str,
    identity: &super::Identity,
) -> Result<bool> {
    let state = sign_in.state.as_str();
    match scope {
        Scope::Instance => app.node()?.sso_answered(sign_in, code_hash, identity).await,
        Scope::Server(id) => {
            let sdb = app.servers.get(id).await?;
            sdb.write("", async |conn, _| super::answered(conn, state, code_hash, identity).await).await
        }
    }
}

/// What a browser may be told about a refusal: the request-level ones are
/// meant for people; anything internal stays in the log.
fn public_message(err: &Error) -> String {
    match err {
        Error::InvalidArgument(m)
        | Error::PermissionDenied(m)
        | Error::FailedPrecondition(m)
        | Error::Unavailable(m) => m.chars().take(300).collect(),
        Error::NotFound(what) => format!("{what} not found"),
        _ => "something went wrong on this instance; try again".into(),
    }
}

fn misrouted() -> Response {
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, "that server is on another shard\n").into_response();
    response.headers_mut().insert(MISROUTED, HeaderValue::from_static("1"));
    response
}

fn refused_page(err: Error) -> Response {
    let status = match err {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    let message = super::xml::escape(&public_message(&err));
    let page = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Single sign-on</title>\
         <body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem;line-height:1.5\">\
         <h1 style=\"font-size:1.25rem\">Signing in didn't work</h1><p>{message}</p><p>Close this tab and try again from the app.</p>"
    );
    let mut response = (status, page).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    response
}
