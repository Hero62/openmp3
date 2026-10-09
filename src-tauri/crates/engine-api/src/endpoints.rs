//! **Every** Spotify internal endpoint lives in this file. When Spotify
//! changes something, this is the only place to patch.
//!
//! Transport notes (see docs/internal-endpoints.md):
//! * reads go through `SpClient::request*` (adds login5 bearer + client-token,
//!   retries on 5xx);
//! * mutations go through [`Endpoints::once`] — a single attempt that also
//!   returns the real status, because SpClient may resend a POST 10 times;
//! * pathfinder (GraphQL) is a hand-built request to api-partner with the same
//!   tokens. Its persisted-query hashes rotate; they're kept in [`HASHES`] and
//!   can be overridden at runtime.

use std::{collections::HashMap, sync::RwLock};

use anyhow::{anyhow, bail, Context as _, Result};
use bytes::Bytes;
use http::{header, HeaderMap, HeaderValue, Method, Request};
use http_body_util::BodyExt;
use librespot_core::{session::Session, SpotifyId, SpotifyUri};
use librespot_protocol as proto;
use log::{debug, warn};
use proto::extended_metadata::{BatchedEntityRequest, EntityRequest, ExtensionQuery};
use proto::extension_kind::ExtensionKind;
pub use proto::playlist4_external as pl;
use protobuf::Message;
use serde_json::{json, Value};

const PATHFINDER_URL: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
const WEB_APP_VERSION: &str = "1.3.5.193.g5b0b36b9d478";

/// Pathfinder persisted-query hashes (web player build of 2026-10-07).
pub const HASHES: &[(&str, &str)] = &[
    ("searchDesktop", "eef7cc54888d91bdd6802623477873caa3948ae173a0c34fd86827b267e94c03"),
    ("searchTracks", "b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751"),
    ("home", "5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1"),
    ("homeSection", "5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1"),
    ("queryArtistOverview", "9f8134ef565e78621f1e1793555bd6633c5ac144ae0f89604ed3ae3f80b3c8e6"),
    ("queryArtistRelated", "3d031d6cb22a2aa7c8d203d49b49df731f58b1e2799cc38d9876d58771aa66f3"),
    ("queryArtistDiscographyAll", "5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599"),
    ("getAlbum", "6a74b456cd1735c9193d9e8ec8cc5184cad7ce13572210315229db3975964361"),
    ("libraryV3", "390c78e5b951029bad359785e69b07b536a509c581cbcd0aded5e5067f187455"),
    ("queryPodcastEpisodes", "3539d746cf882f3909660de40b4ef472b3f5893a0761d617c282541de5d412c5"),
    ("queryShowMetadataV2", "b475447846f37cc426add800b995a7859c169c53c72b62de6338c0c40a5dacea"),
];

pub struct Endpoints {
    session: Session,
    hash_overrides: RwLock<HashMap<String, String>>,
}

pub struct RawResponse {
    pub status: u16,
    pub body: Bytes,
}

fn json_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    h
}

/// 16-byte gid → base62 id.
pub fn gid_to_b62(gid: &[u8]) -> Option<String> {
    Some(SpotifyId::from_raw(gid).ok()?.to_base62())
}

pub fn uri_b62(uri: &str) -> Result<String> {
    uri.rsplit(':').next().filter(|s| s.len() == 22).map(String::from).ok_or_else(|| anyhow!("bad uri {uri}"))
}

impl Endpoints {
    pub fn new(session: Session) -> Self {
        Self { session, hash_overrides: RwLock::new(HashMap::new()) }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn username(&self) -> String {
        self.session.username()
    }

    pub fn country(&self) -> String {
        self.session.country()
    }

    pub fn set_hash(&self, op: &str, hash: &str) {
        self.hash_overrides.write().unwrap().insert(op.to_string(), hash.to_string());
    }

    fn hash(&self, op: &str) -> Option<String> {
        if let Some(h) = self.hash_overrides.read().unwrap().get(op) {
            return Some(h.clone());
        }
        HASHES.iter().find(|(o, _)| *o == op).map(|(_, h)| h.to_string())
    }

    // ---------------------------------------------------------------- transport

    async fn sp_get_json(&self, path: &str, extra: Option<HeaderMap>) -> Result<Value> {
        let bytes = self
            .session
            .spclient()
            .request_as_json(&Method::GET, path, extra, None)
            .await
            .with_context(|| format!("GET {}", path.split('?').next().unwrap_or(path)))?;
        serde_json::from_slice(&bytes).context("json")
    }

    /// Idempotent JSON POST (reads like paging/contains) — retries are fine.
    async fn sp_post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let body = serde_json::to_vec(body)?;
        let bytes = self
            .session
            .spclient()
            .request(&Method::POST, path, Some(json_headers()), Some(&body))
            .await
            .with_context(|| format!("POST {path}"))?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).context("json")
    }

    async fn sp_get_proto(&self, path: &str) -> Result<Bytes> {
        self.session
            .spclient()
            .request(&Method::GET, path, None, None)
            .await
            .with_context(|| format!("GET {}", path.split('?').next().unwrap_or(path)))
    }

    /// One attempt, real status code, body kept (for mutations).
    pub async fn once(&self, method: Method, url: &str, content_type: &str, body: Vec<u8>) -> Result<RawResponse> {
        let accept = if content_type.contains("protobuf") { "application/x-protobuf" } else { "application/json" };
        let tok = self.session.login5().auth_token().await.map_err(|e| anyhow!("login5: {e}"))?;
        let ct = self.session.spclient().client_token().await.unwrap_or_default();
        let mut req = Request::builder()
            .method(method)
            .uri(url)
            .header(header::AUTHORIZATION, format!("Bearer {}", tok.access_token))
            .header(header::CONTENT_TYPE, content_type)
            .header(header::ACCEPT, accept);
        if !ct.is_empty() {
            req = req.header("client-token", ct);
        }
        if url.starts_with(PATHFINDER_URL) {
            req = req
                .header("app-platform", "WebPlayer")
                .header("spotify-app-version", WEB_APP_VERSION)
                .header(header::ACCEPT_LANGUAGE, "en");
        }
        let req = req.body(Bytes::from(body))?;
        let resp = self.session.http_client().request_fut(req).map_err(|e| anyhow!("http: {e}"))?.await?;
        let status = resp.status().as_u16();
        let body = resp.into_body().collect().await?.to_bytes();
        Ok(RawResponse { status, body })
    }

    async fn sp_once_json(&self, path: &str, body: &Value) -> Result<Value> {
        let base = self.session.spclient().base_url().await.map_err(|e| anyhow!("spclient: {e}"))?;
        let url = format!("{base}{path}");
        let r = self.once(Method::POST, &url, "application/json", serde_json::to_vec(body)?).await?;
        if !(200..300).contains(&r.status) {
            bail!("POST {path} → HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().take(300).collect::<String>());
        }
        if r.body.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&r.body).unwrap_or(Value::Null))
    }

    /// Pathfinder GraphQL persisted query.
    pub async fn pathfinder(&self, op: &str, variables: Value) -> Result<Value> {
        let hash = self.hash(op).ok_or_else(|| anyhow!("no hash for {op}"))?;
        let body = json!({
            "variables": variables,
            "operationName": op,
            "extensions": { "persistedQuery": { "version": 1, "sha256Hash": hash } }
        });
        let r = self.once(Method::POST, PATHFINDER_URL, "application/json;charset=UTF-8", serde_json::to_vec(&body)?).await?;
        if r.status == 401 || r.status == 403 {
            bail!("pathfinder {op}: HTTP {} (token rejected)", r.status);
        }
        if !(200..300).contains(&r.status) {
            bail!("pathfinder {op}: HTTP {}", r.status);
        }
        let v: Value = serde_json::from_slice(&r.body).context("pathfinder json")?;
        if let Some(errs) = v.get("errors").and_then(|e| e.as_array()) {
            let msg = errs.iter().filter_map(|e| e.get("message").and_then(|m| m.as_str())).collect::<Vec<_>>().join("; ");
            if msg.contains("PersistedQueryNotFound") {
                bail!("pathfinder {op}: hash rotated (PersistedQueryNotFound)");
            }
            if v.get("data").map(|d| d.is_null()).unwrap_or(true) {
                bail!("pathfinder {op}: {msg}");
            }
            warn!("pathfinder {op} partial errors: {msg}");
        }
        Ok(v)
    }

    // ---------------------------------------------------------------- playlists

    pub async fn rootlist(&self) -> Result<proto::playlist4_external::SelectedListContent> {
        let user = self.username();
        let path = format!(
            "/playlist/v2/user/{user}/rootlist?decorate=revision,length,attributes,timestamp,owner,capabilities"
        );
        let bytes = self.sp_get_proto(&path).await?;
        Ok(proto::playlist4_external::SelectedListContent::parse_from_bytes(&bytes)?)
    }

    pub async fn playlist(&self, b62: &str) -> Result<proto::playlist4_external::SelectedListContent> {
        let path = format!("/playlist/v2/playlist/{b62}?decorate=revision,length,attributes,timestamp,owner,capabilities");
        let bytes = self.sp_get_proto(&path).await?;
        Ok(proto::playlist4_external::SelectedListContent::parse_from_bytes(&bytes)?)
    }

    pub async fn playlist_revision(&self, b62: &str) -> Result<Vec<u8>> {
        let path = format!("/playlist/v2/playlist/{b62}?decorate=revision&from=0&length=1");
        let bytes = self.sp_get_proto(&path).await?;
        let c = proto::playlist4_external::SelectedListContent::parse_from_bytes(&bytes)?;
        Ok(c.revision.unwrap_or_default())
    }

    // ---------------------------------------------------------------- playlist writes (protobuf)

    async fn sp_once_proto(&self, path: &str, body: Vec<u8>) -> Result<Bytes> {
        let base = self.session.spclient().base_url().await.map_err(|e| anyhow!("spclient: {e}"))?;
        let url = format!("{base}{path}");
        let r = self.once(Method::POST, &url, "application/x-protobuf", body).await?;
        if !(200..300).contains(&r.status) {
            bail!("POST {path} → HTTP {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().take(300).collect::<String>());
        }
        Ok(r.body)
    }

    /// Create an (empty) playlist; returns its uri. Caller adds it to the rootlist.
    pub async fn playlist_create(&self, name: &str) -> Result<String> {
        let mut attrs = pl::ListAttributes::new();
        attrs.name = Some(name.to_string());
        let mut d = pl::Delta::new();
        d.ops.push(pl_ops::update_attrs(attrs));
        d.info = protobuf::MessageField::some(pl_ops::info(&self.username()));
        let body = self.sp_once_proto("/playlist/v2/playlist", d.write_to_bytes()?).await?;
        let reply = pl::CreateListReply::parse_from_bytes(&body)?;
        reply.uri.ok_or_else(|| anyhow!("create: no uri in reply"))
    }

    /// Apply ops to a playlist (`base_revision` needed for index-based ops).
    pub async fn playlist_changes(&self, b62: &str, ops: Vec<pl::Op>, base_revision: Option<Vec<u8>>) -> Result<()> {
        let mut d = pl::Delta::new();
        d.ops = ops;
        d.info = protobuf::MessageField::some(pl_ops::info(&self.username()));
        let mut lc = pl::ListChanges::new();
        lc.base_revision = base_revision;
        lc.deltas.push(d);
        self.sp_once_proto(&format!("/playlist/v2/playlist/{b62}/changes"), lc.write_to_bytes()?).await?;
        Ok(())
    }

    pub async fn rootlist_changes(&self, ops: Vec<pl::Op>) -> Result<()> {
        let user = self.username();
        let mut d = pl::Delta::new();
        d.ops = ops;
        d.info = protobuf::MessageField::some(pl_ops::info(&user));
        let mut lc = pl::ListChanges::new();
        lc.deltas.push(d);
        lc.want_resulting_revisions = Some(false);
        lc.want_sync_result = Some(false);
        self.sp_once_proto(&format!("/playlist/v2/user/{user}/rootlist/changes"), lc.write_to_bytes()?).await?;
        Ok(())
    }

    // ---------------------------------------------------------------- metadata

    /// Batch extended metadata. Returns (entity_uri, raw message bytes) for
    /// entries with status 200.
    pub async fn extended_metadata(&self, uris: &[String], kind: ExtensionKind) -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::with_capacity(uris.len());
        for chunk in uris.chunks(100) {
            let mut req = BatchedEntityRequest::new();
            for u in chunk {
                let mut q = ExtensionQuery::new();
                q.extension_kind = kind.into();
                let mut er = EntityRequest::new();
                er.entity_uri = u.clone();
                er.query.push(q);
                req.entity_request.push(er);
            }
            let resp = self.session.spclient().get_extended_metadata(req).await.map_err(|e| anyhow!("extended-metadata: {e}"))?;
            for arr in resp.extended_metadata {
                for d in arr.extension_data {
                    let status = d.header.as_ref().map(|h| h.status_code).unwrap_or(0);
                    if status != 200 {
                        continue;
                    }
                    if let Some(any) = d.extension_data.as_ref() {
                        out.push((d.entity_uri.clone(), any.value.clone()));
                    }
                }
            }
        }
        Ok(out)
    }

    pub async fn tracks_meta(&self, uris: &[String]) -> Result<Vec<(String, proto::metadata::Track)>> {
        let raw = self.extended_metadata(uris, ExtensionKind::TRACK_V4).await?;
        Ok(raw.into_iter().filter_map(|(u, b)| Some((u, proto::metadata::Track::parse_from_bytes(&b).ok()?))).collect())
    }

    pub async fn episodes_meta(&self, uris: &[String]) -> Result<Vec<(String, proto::metadata::Episode)>> {
        let raw = self.extended_metadata(uris, ExtensionKind::EPISODE_V4).await?;
        Ok(raw.into_iter().filter_map(|(u, b)| Some((u, proto::metadata::Episode::parse_from_bytes(&b).ok()?))).collect())
    }

    pub async fn albums_meta(&self, uris: &[String]) -> Result<Vec<(String, proto::metadata::Album)>> {
        let raw = self.extended_metadata(uris, ExtensionKind::ALBUM_V4).await?;
        Ok(raw.into_iter().filter_map(|(u, b)| Some((u, proto::metadata::Album::parse_from_bytes(&b).ok()?))).collect())
    }

    pub async fn artists_meta(&self, uris: &[String]) -> Result<Vec<(String, proto::metadata::Artist)>> {
        let raw = self.extended_metadata(uris, ExtensionKind::ARTIST_V4).await?;
        Ok(raw.into_iter().filter_map(|(u, b)| Some((u, proto::metadata::Artist::parse_from_bytes(&b).ok()?))).collect())
    }

    pub async fn shows_meta(&self, uris: &[String]) -> Result<Vec<(String, proto::metadata::Show)>> {
        let raw = self.extended_metadata(uris, ExtensionKind::SHOW_V4).await?;
        Ok(raw.into_iter().filter_map(|(u, b)| Some((u, proto::metadata::Show::parse_from_bytes(&b).ok()?))).collect())
    }

    pub async fn show_episode_uris(&self, show_uri: &str) -> Result<Vec<String>> {
        for kind in [ExtensionKind::SHOW_V4_EPISODES_ASSOC, ExtensionKind::SHOW_EPISODES_ASSOC] {
            let raw = self.extended_metadata(&[show_uri.to_string()], kind).await?;
            if let Some((_, b)) = raw.into_iter().next() {
                if let Ok(a) = proto::entity_extension_data::Assoc::parse_from_bytes(&b) {
                    let list: Vec<String> = a.plain_list.entity_uri.clone();
                    if !list.is_empty() {
                        return Ok(list);
                    }
                }
            }
        }
        Ok(Vec::new())
    }

    // ---------------------------------------------------------------- collection

    /// All items of a collection set (`collection` = liked tracks + saved albums,
    /// `artist`, `show`, …). Returns (uri, added_at seconds).
    pub async fn collection(&self, set: &str) -> Result<Vec<(String, i64)>> {
        let user = self.username();
        let mut out = Vec::new();
        let mut token = String::new();
        for _ in 0..200 {
            let v = self
                .sp_post_json(
                    "/collection/v2/paging",
                    &json!({ "username": user, "set": set, "limit": 300, "pagination_token": token }),
                )
                .await?;
            if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
                for it in items {
                    let uri = it.get("uri").and_then(|u| u.as_str()).unwrap_or_default();
                    if uri.is_empty() {
                        continue;
                    }
                    let added = it
                        .get("added_at")
                        .or_else(|| it.get("addedAt"))
                        .and_then(|a| a.as_i64().or_else(|| a.as_str().and_then(|s| s.parse().ok())))
                        .unwrap_or(0);
                    out.push((uri.to_string(), added));
                }
            }
            token = v
                .get("next_page_token")
                .or_else(|| v.get("nextPageToken"))
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            if token.is_empty() {
                break;
            }
        }
        Ok(out)
    }

    pub async fn collection_write(&self, set: &str, uris: &[String], remove: bool) -> Result<()> {
        let user = self.username();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let items: Vec<Value> = uris
            .iter()
            .map(|u| if remove { json!({ "uri": u, "is_removed": true }) } else { json!({ "uri": u, "added_at": now }) })
            .collect();
        self.sp_once_json("/collection/v2/write", &json!({ "username": user, "set": set, "items": items })).await?;
        Ok(())
    }

    pub async fn collection_contains(&self, set: &str, uris: &[String]) -> Result<Vec<bool>> {
        let user = self.username();
        let items: Vec<Value> = uris.iter().map(|u| json!({ "uri": u })).collect();
        let v = self.sp_post_json("/collection/v2/contains", &json!({ "username": user, "set": set, "items": items })).await?;
        let found = v.get("found").and_then(|f| f.as_array()).cloned().unwrap_or_default();
        Ok(uris.iter().enumerate().map(|(i, _)| found.get(i).and_then(|b| b.as_bool()).unwrap_or(false)).collect())
    }

    // ---------------------------------------------------------------- contexts / radio

    pub async fn context(&self, uri: &str) -> Result<proto::context::Context> {
        self.session.spclient().get_context(uri).await.map_err(|e| anyhow!("context-resolve {uri}: {e}"))
    }

    pub async fn next_page(&self, url: &str) -> Result<proto::context_page::ContextPage> {
        let bytes = self.session.spclient().get_next_page(url).await.map_err(|e| anyhow!("next page: {e}"))?;
        let v: Value = serde_json::from_slice(&bytes)?;
        let s = serde_json::to_string(&v)?;
        protobuf_json_mapping::parse_from_str(&s).map_err(|e| anyhow!("page json: {e}"))
    }

    pub async fn autoplay(&self, context_uri: &str, recent: &[String]) -> Result<proto::context::Context> {
        let mut req = proto::autoplay_context_request::AutoplayContextRequest::new();
        req.context_uri = Some(context_uri.to_string());
        req.recent_track_uri = recent.to_vec();
        req.is_video = Some(false);
        self.session.spclient().get_autoplay_context(&req).await.map_err(|e| anyhow!("autoplay: {e}"))
    }

    pub async fn radio_for(&self, uri: &str) -> Result<Vec<String>> {
        let u = SpotifyUri::from_uri(uri).map_err(|e| anyhow!("{e}"))?;
        let bytes = self.session.spclient().get_radio_for_track(&u).await.map_err(|e| anyhow!("radio: {e}"))?;
        let v: Value = serde_json::from_slice(&bytes)?;
        Ok(v.get("mediaItems")
            .and_then(|m| m.as_array())
            .map(|a| a.iter().filter_map(|i| i.get("uri").and_then(|u| u.as_str()).map(String::from)).collect())
            .unwrap_or_default())
    }

    // ---------------------------------------------------------------- lyrics

    pub async fn lyrics(&self, track_b62: &str) -> Result<Option<Value>> {
        let mut h = HeaderMap::new();
        h.insert("app-platform", HeaderValue::from_static("WebPlayer"));
        let path = format!("/color-lyrics/v2/track/{track_b62}?format=json&vocalRemoval=false&market=from_token");
        match self.sp_get_json(&path, Some(h)).await {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                let s = format!("{e:#}");
                if s.contains("NotFound") || s.contains("404") || s.contains("not found") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    // ---------------------------------------------------------------- pathfinder views

    pub async fn search(&self, q: &str, limit: u32) -> Result<Value> {
        self.pathfinder(
            "searchDesktop",
            json!({
                "searchTerm": q, "offset": 0, "limit": limit, "numberOfTopResults": 5,
                "includeAudiobooks": true, "includePreReleases": false, "includeAlbumPreReleases": false,
                "includeAuthors": false, "includeEpisodeContentRatingsV2": false
            }),
        )
        .await
    }

    pub async fn home(&self, time_zone: &str) -> Result<Value> {
        let vars = json!({
            "timeZone": time_zone, "sp_t": "", "facet": "", "sectionItemsLimit": 12,
            "includeHomeChapterVideoCards": false, "homeEndUserIntegration": "INTEGRATION_WEB_PLAYER",
            "includeEpisodeContentRatingsV2": false
        });
        match self.pathfinder("home", vars.clone()).await {
            Ok(v) => Ok(v),
            Err(e) => {
                debug!("home with integration failed ({e}); retrying without");
                let mut v2 = vars;
                v2.as_object_mut().unwrap().remove("homeEndUserIntegration");
                self.pathfinder("home", v2).await
            }
        }
    }

    pub async fn artist_overview(&self, uri: &str) -> Result<Value> {
        self.pathfinder("queryArtistOverview", json!({ "uri": uri, "locale": "", "preReleaseV2": false })).await
    }

    pub async fn recently_played(&self) -> Result<Value> {
        let user = self.username();
        self.sp_get_json(
            &format!("/recently-played/v3/user/{user}/recently-played?format=json&offset=0&limit=50&filter=default,collection-new-episodes"),
            None,
        )
        .await
    }
}

/// Builders for playlist4 ops.
pub mod pl_ops {
    use super::pl;
    use protobuf::{EnumOrUnknown, MessageField};

    fn now_ms() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
    }

    pub fn info(user: &str) -> pl::ChangeInfo {
        let mut src = pl::SourceInfo::new();
        src.client = Some(EnumOrUnknown::new(pl::source_info::Client::CLIENT));
        let mut i = pl::ChangeInfo::new();
        i.user = Some(user.to_string());
        i.timestamp = Some(now_ms());
        i.source = MessageField::some(src);
        i
    }

    fn op(kind: pl::op::Kind) -> pl::Op {
        let mut o = pl::Op::new();
        o.kind = Some(EnumOrUnknown::new(kind));
        o
    }

    pub fn item(uri: &str, with_timestamp: bool) -> pl::Item {
        let mut it = pl::Item::new();
        it.uri = Some(uri.to_string());
        if with_timestamp {
            let mut a = pl::ItemAttributes::new();
            a.timestamp = Some(now_ms());
            it.attributes = MessageField::some(a);
        }
        it
    }

    pub fn update_attrs(values: pl::ListAttributes) -> pl::Op {
        let mut ps = pl::ListAttributesPartialState::new();
        ps.values = MessageField::some(values);
        let mut u = pl::UpdateListAttributes::new();
        u.new_attributes = MessageField::some(ps);
        let mut o = op(pl::op::Kind::UPDATE_LIST_ATTRIBUTES);
        o.update_list_attributes = MessageField::some(u);
        o
    }

    pub enum Pos {
        First,
        Last,
        Index(i32),
    }

    pub fn add(uris: &[String], pos: Pos, ts: bool) -> pl::Op {
        let mut a = pl::Add::new();
        a.items = uris.iter().map(|u| item(u, ts)).collect();
        match pos {
            Pos::First => a.add_first = Some(true),
            Pos::Last => a.add_last = Some(true),
            Pos::Index(i) => a.from_index = Some(i),
        }
        let mut o = op(pl::op::Kind::ADD);
        o.add = MessageField::some(a);
        o
    }

    pub fn rem_index(from: i32, length: i32) -> pl::Op {
        let mut r = pl::Rem::new();
        r.from_index = Some(from);
        r.length = Some(length);
        let mut o = op(pl::op::Kind::REM);
        o.rem = MessageField::some(r);
        o
    }

    pub fn rem_uris(uris: &[String]) -> pl::Op {
        let mut r = pl::Rem::new();
        r.items = uris.iter().map(|u| item(u, false)).collect();
        r.items_as_key = Some(true);
        let mut o = op(pl::op::Kind::REM);
        o.rem = MessageField::some(r);
        o
    }

    pub fn mov(from: i32, length: i32, to: i32) -> pl::Op {
        let mut m = pl::Mov::new();
        m.from_index = Some(from);
        m.length = Some(length);
        m.to_index = Some(to);
        let mut o = op(pl::op::Kind::MOV);
        o.mov = MessageField::some(m);
        o
    }
}
