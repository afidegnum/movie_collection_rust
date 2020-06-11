#![allow(clippy::needless_pass_by_value)]

use actix_web::{
    web::{Data, Json, Path, Query},
    HttpResponse,
};
use anyhow::format_err;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::hash::{Hash, Hasher};
use std::{
    collections::{HashMap, HashSet},
    path,
};
use subprocess::Exec;

use movie_collection_lib::{
    make_queue::movie_queue_http,
    movie_collection::{ImdbSeason, TvShowsResult},
    movie_queue::MovieQueueResult,
    pgpool::PgPool,
    stack_string::StackString,
    stdout_channel::StdoutChannel,
    trakt_utils::{TraktActions, WatchListShow, TRAKT_CONN},
    tv_show_source::TvShowSource,
    utils::remcom_single_file,
};

use super::{
    errors::ServiceError as Error,
    logged_user::LoggedUser,
    movie_queue_app::AppState,
    movie_queue_requests::{
        FindNewEpisodeRequest, ImdbEpisodesSyncRequest, ImdbEpisodesUpdateRequest,
        ImdbRatingsRequest, ImdbRatingsSyncRequest, ImdbRatingsUpdateRequest, ImdbSeasonsRequest,
        ImdbShowRequest, LastModifiedRequest, MovieCollectionSyncRequest,
        MovieCollectionUpdateRequest, MoviePathRequest, MovieQueueRequest, MovieQueueSyncRequest,
        MovieQueueUpdateRequest, ParseImdbRequest, QueueDeleteRequest, TraktCalRequest,
        TvShowsRequest, WatchedActionRequest, WatchedListRequest, WatchlistActionRequest,
        WatchlistShowsRequest,
    },
    HandleRequest,
};

fn form_http_response(body: String) -> Result<HttpResponse, Error> {
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(body))
}

fn to_json<T>(js: T) -> Result<HttpResponse, Error>
where
    T: Serialize,
{
    Ok(HttpResponse::Ok().json(js))
}

fn movie_queue_body(patterns: &[StackString], entries: &[String]) -> String {
    let previous = r#"<a href="javascript:updateMainArticle('/list/tvshows')">Go Back</a><br>"#;

    let watchlist_url = if patterns.is_empty() {
        "/list/trakt/watchlist".to_string()
    } else {
        format!("/list/trakt/watched/list/{}", patterns.join("_"))
    };

    let entries = format!(
        r#"{}<a href="javascript:updateMainArticle('{}')">Watch List</a><table border="0">{}</table>"#,
        previous,
        watchlist_url,
        entries.join("")
    );

    entries
}

async fn queue_body_resp(
    patterns: Vec<StackString>,
    queue: Vec<MovieQueueResult>,
    pool: &PgPool,
) -> Result<HttpResponse, Error> {
    let entries = movie_queue_http(&queue, pool).await?;
    let body = movie_queue_body(&patterns, &entries);
    form_http_response(body)
}

pub async fn movie_queue(_: LoggedUser, state: Data<AppState>) -> Result<HttpResponse, Error> {
    let req = MovieQueueRequest {
        patterns: Vec::new(),
    };
    let (queue, _) = state.db.handle(req).await?;
    queue_body_resp(Vec::new(), queue, &state.db).await
}

pub async fn movie_queue_show(
    path: Path<String>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let path = path.into_inner();
    let patterns = vec![path.into()];

    let req = MovieQueueRequest { patterns };
    let (queue, patterns) = state.db.handle(req).await?;
    queue_body_resp(patterns, queue, &state.db).await
}

pub async fn movie_queue_delete(
    path: Path<String>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let path = path.into_inner().into();

    let req = QueueDeleteRequest { path };
    let body = state.db.handle(req).await?;
    form_http_response(body.into())
}

fn transcode_worker(
    directory: Option<&path::Path>,
    entries: &[MovieQueueResult],
    stdout: &StdoutChannel,
) -> Result<HttpResponse, Error> {
    let entries: Result<Vec<_>, Error> = entries
        .iter()
        .map(|entry| {
            remcom_single_file(
                &path::Path::new(entry.path.as_str()),
                directory,
                false,
                &stdout,
            )?;
            Ok(format!("{}", entry))
        })
        .collect();
    form_http_response(entries?.join(""))
}

pub async fn movie_queue_transcode(
    path: Path<String>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let path = path.into_inner().into();
    let patterns = vec![path];

    let req = MovieQueueRequest { patterns };
    let (entries, _) = state.db.handle(req).await?;
    let stdout = StdoutChannel::new();
    transcode_worker(None, &entries, &stdout)
}

pub async fn movie_queue_transcode_directory(
    path: Path<(String, String)>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let (directory, file) = path.into_inner();
    let patterns = vec![file.into()];

    let req = MovieQueueRequest { patterns };
    let (entries, _) = state.db.handle(req).await?;
    let stdout = StdoutChannel::new();
    transcode_worker(
        Some(&path::Path::new(directory.as_str())),
        &entries,
        &stdout,
    )
}

fn play_worker(full_path: String) -> Result<HttpResponse, Error> {
    let path = path::Path::new(&full_path);

    let file_name = path
        .file_name()
        .ok_or_else(|| format_err!("Invalid path"))?
        .to_string_lossy();
    let url = format!("/videos/partial/{}", file_name);

    let body = format!(
        r#"
        {}<br>
        <video width="720" controls>
        <source src="{}" type="video/mp4">
        Your browser does not support HTML5 video.
        </video>
    "#,
        file_name, url
    );

    let command = format!("rm -f /var/www/html/videos/partial/{}", file_name);
    Exec::shell(&command).join()?;
    let command = format!(
        "ln -s {} /var/www/html/videos/partial/{}",
        full_path, file_name
    );
    Exec::shell(&command).join()?;
    form_http_response(body)
}

pub async fn movie_queue_play(
    idx: Path<i32>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let idx = idx.into_inner();

    let req = MoviePathRequest { idx };
    let x = state.db.handle(req).await?;
    play_worker(x)
}

pub async fn imdb_show(
    path: Path<String>,
    query: Query<ParseImdbRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let show = path.into_inner().into();
    let query = query.into_inner();

    let req = ImdbShowRequest { show, query };
    let x = state.db.handle(req).await?;
    form_http_response(x)
}

fn new_episode_worker(entries: &[String]) -> Result<HttpResponse, Error> {
    let previous = r#"
        <a href="javascript:updateMainArticle('/list/tvshows')">Go Back</a><br>
        <input type="button" name="list_cal" value="TVCalendar" onclick="updateMainArticle('/list/cal');"/>
        <input type="button" name="list_cal" value="NetflixCalendar" onclick="updateMainArticle('/list/cal?source=netflix');"/>
        <input type="button" name="list_cal" value="AmazonCalendar" onclick="updateMainArticle('/list/cal?source=amazon');"/>
        <input type="button" name="list_cal" value="HuluCalendar" onclick="updateMainArticle('/list/cal?source=hulu');"/><br>
        <button name="remcomout" id="remcomoutput"> &nbsp; </button>
    "#;
    let entries = format!(
        r#"{}<table border="0">{}</table>"#,
        previous,
        entries.join("")
    );
    form_http_response(entries)
}

pub async fn find_new_episodes(
    query: Query<FindNewEpisodeRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let req = query.into_inner();
    let entries = state.db.handle(req).await?;
    new_episode_worker(&entries)
}

pub async fn imdb_episodes_route(
    query: Query<ImdbEpisodesSyncRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let req = query.into_inner();
    let x = state.db.handle(req).await?;
    to_json(x)
}

pub async fn imdb_episodes_update(
    data: Json<ImdbEpisodesUpdateRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let episodes = data.into_inner();

    let req = episodes;
    state.db.handle(req).await?;
    form_http_response("Success".to_string())
}

pub async fn imdb_ratings_route(
    query: Query<ImdbRatingsSyncRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let req = query.into_inner();
    let x = state.db.handle(req).await?;
    to_json(x)
}

pub async fn imdb_ratings_update(
    data: Json<ImdbRatingsUpdateRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let shows = data.into_inner();

    let req = shows;
    state.db.handle(req).await?;
    form_http_response("Success".to_string())
}

pub async fn movie_queue_route(
    query: Query<MovieQueueSyncRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let req = query.into_inner();
    let x = state.db.handle(req).await?;
    to_json(x)
}

pub async fn movie_queue_update(
    data: Json<MovieQueueUpdateRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let queue = data.into_inner();

    let req = queue;
    state.db.handle(req).await?;
    form_http_response("Success".to_string())
}

pub async fn movie_collection_route(
    query: Query<MovieCollectionSyncRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let req = query.into_inner();
    let x = state.db.handle(req).await?;
    to_json(x)
}

pub async fn movie_collection_update(
    data: Json<MovieCollectionUpdateRequest>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let collection = data.into_inner();

    let req = collection;
    state.db.handle(req).await?;
    form_http_response("Success".to_string())
}

pub async fn last_modified_route(
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let req = LastModifiedRequest {};
    let x = state.db.handle(req).await?;
    to_json(x)
}

pub async fn frontpage(_: LoggedUser, _: Data<AppState>) -> Result<HttpResponse, Error> {
    form_http_response(include_str!("../../templates/index.html").replace("BODY", ""))
}

type TvShowsMap = HashMap<StackString, (StackString, WatchListShow, Option<TvShowSource>)>;

#[derive(Debug, Default, Eq)]
struct ProcessShowItem {
    show: StackString,
    title: StackString,
    link: StackString,
    source: Option<TvShowSource>,
}

impl PartialEq for ProcessShowItem {
    fn eq(&self, other: &Self) -> bool {
        self.link == other.link
    }
}

impl Hash for ProcessShowItem {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.link.hash(state)
    }
}

impl Borrow<str> for ProcessShowItem {
    fn borrow(&self) -> &str {
        self.link.as_str()
    }
}

impl From<TvShowsResult> for ProcessShowItem {
    fn from(item: TvShowsResult) -> Self {
        Self {
            show: item.show,
            title: item.title,
            link: item.link,
            source: item.source,
        }
    }
}

fn tvshows_worker(res1: TvShowsMap, tvshows: Vec<TvShowsResult>) -> Result<String, Error> {
    let tvshows: HashSet<_> = tvshows
        .into_iter()
        .map(|s| {
            let item: ProcessShowItem = s.into();
            item
        })
        .collect();
    let watchlist: HashSet<_> = res1
        .into_iter()
        .map(|(link, (show, s, source))| {
            let item = ProcessShowItem {
                show,
                title: s.title,
                link: s.link,
                source,
            };
            debug_assert!(link.as_str() == item.link.as_str());
            item
        })
        .collect();

    let shows = process_shows(tvshows, watchlist)?;

    let previous = r#"
        <a href="javascript:updateMainArticle('/list/watchlist')">Go Back</a><br>
        <a href="javascript:updateMainArticle('/list/trakt/watchlist')">Watch List</a>
        <button name="remcomout" id="remcomoutput"> &nbsp; </button><br>
    "#;

    let entries = format!(
        r#"{}<table border="0">{}</table>"#,
        previous,
        shows.join("")
    );

    Ok(entries)
}

pub async fn tvshows(_: LoggedUser, state: Data<AppState>) -> Result<HttpResponse, Error> {
    let s = state.clone();
    let shows = s.db.handle(TvShowsRequest {}).await?;
    let res1 = state.db.handle(WatchlistShowsRequest {}).await?;
    let entries = tvshows_worker(res1, shows)?;
    form_http_response(entries)
}

fn process_shows(
    tvshows: HashSet<ProcessShowItem>,
    watchlist: HashSet<ProcessShowItem>,
) -> Result<Vec<String>, Error> {
    let watchlist_shows: Vec<_> = watchlist
        .iter()
        .filter_map(|item| match tvshows.get(item.link.as_str()) {
            None => Some(item),
            Some(_) => None,
        })
        .collect();

    let mut shows: Vec<_> = tvshows.iter().chain(watchlist_shows.into_iter()).collect();
    shows.sort_by(|x, y| x.show.cmp(&y.show));

    let button_add = r#"<td><button type="submit" id="ID" onclick="watchlist_add('SHOW');">add to watchlist</button></td>"#;
    let button_rm = r#"<td><button type="submit" id="ID" onclick="watchlist_rm('SHOW');">remove from watchlist</button></td>"#;

    let shows: Vec<_> = shows
        .into_iter()
        .map(|item| {
            let has_watchlist = watchlist.contains(item.link.as_str());
            format!(
                r#"<tr><td>{}</td>
                <td><a href="https://www.imdb.com/title/{}" target="_blank">imdb</a></td><td>{}</td><td>{}</td><td>{}</td></tr>"#,
                if tvshows.contains(item.link.as_str()) {
                    format!(r#"<a href="javascript:updateMainArticle('/list/{}')">{}</a>"#, item.show, item.title)
                } else {
                    format!(
                        r#"<a href="javascript:updateMainArticle('/list/trakt/watched/list/{}')">{}</a>"#,
                        item.link, item.title
                    )
                },
                item.link,
                match item.source {
                    Some(TvShowSource::Netflix) => r#"<a href="https://netflix.com" target="_blank">netflix</a>"#,
                    Some(TvShowSource::Hulu) => r#"<a href="https://hulu.com" target="_blank">hulu</a>"#,
                    Some(TvShowSource::Amazon) => r#"<a href="https://amazon.com" target="_blank">amazon</a>"#,
                    _ => "",
                },
                if has_watchlist {
                    format!(r#"<a href="javascript:updateMainArticle('/list/trakt/watched/list/{}')">watchlist</a>"#, item.link)
                } else {
                    "".to_string()
                },
                if has_watchlist {
                    button_rm.replace("SHOW", &item.link)
                } else {
                    button_add.replace("SHOW", &item.link)
                },
            )
        })
        .collect();
    Ok(shows)
}

fn watchlist_worker(
    shows: HashMap<StackString, (StackString, WatchListShow, Option<TvShowSource>)>,
) -> Result<HttpResponse, Error> {
    let mut shows: Vec<_> = shows
        .into_iter()
        .map(|(_, (_, s, source))| (s.title, s.link, source))
        .collect();

    shows.sort();

    let shows: Vec<_> = shows
        .into_iter()
        .map(|(title, link, source)| {
            format!(
                r#"<tr><td>{}</td>
            <td><a href="https://www.imdb.com/title/{}" target="_blank">imdb</a> {} </tr>"#,
                format!(
                    r#"<a href="javascript:updateMainArticle('/list/trakt/watched/list/{}')">{}</a>"#,
                    link, title
                ),
                link,
                match source {
                    Some(TvShowSource::Netflix) => {
                        r#"<td><a href="https://netflix.com" target="_blank">netflix</a>"#
                    }
                    Some(TvShowSource::Hulu) => r#"<td><a href="https://hulu.com" target="_blank">netflix</a>"#,
                    Some(TvShowSource::Amazon) => r#"<td><a href="https://amazon.com" target="_blank">netflix</a>"#,
                    _ => "",
                },
            )
        })
        .collect();

    let previous = r#"<a href="javascript:updateMainArticle('/list/tvshows')">Go Back</a><br>"#;
    let entries = format!(
        r#"{}<table border="0">{}</table>"#,
        previous,
        shows.join("")
    );

    form_http_response(entries)
}

pub async fn trakt_watchlist(_: LoggedUser, state: Data<AppState>) -> Result<HttpResponse, Error> {
    let req = WatchlistShowsRequest {};
    let x = state.db.handle(req).await?;
    watchlist_worker(x)
}

async fn watchlist_action_worker(
    action: TraktActions,
    imdb_url: &str,
) -> Result<HttpResponse, Error> {
    TRAKT_CONN.init().await;
    let body = match action {
        TraktActions::Add => TRAKT_CONN.add_watchlist_show(&imdb_url).await?.to_string(),
        TraktActions::Remove => TRAKT_CONN
            .remove_watchlist_show(&imdb_url)
            .await?
            .to_string(),
        _ => "".to_string(),
    };
    form_http_response(body)
}

pub async fn trakt_watchlist_action(
    path: Path<(String, String)>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let (action, imdb_url) = path.into_inner();
    let action = action.parse().expect("impossible");

    let req = WatchlistActionRequest {
        action,
        imdb_url: imdb_url.into(),
    };
    let imdb_url = state.db.handle(req).await?;
    watchlist_action_worker(action, &imdb_url).await
}

fn trakt_watched_seasons_worker(
    link: &str,
    imdb_url: &str,
    entries: &[ImdbSeason],
) -> Result<String, Error> {
    let button_add = r#"
        <td>
        <button type="submit" id="ID"
            onclick="imdb_update('SHOW', 'LINK', SEASON, '/list/trakt/watched/list/LINK');"
            >update database</button></td>"#;

    let entries: Vec<_> = entries
        .iter()
        .map(|s| {
            format!(
                "<tr><td>{}<td>{}<td>{}<td>{}</tr>",
                format!(
                    r#"<a href="javascript:updateMainArticle('/list/trakt/watched/list/{}/{}')">{}</t>"#,
                    imdb_url, s.season, s.title
                ),
                s.season,
                s.nepisodes,
                button_add
                    .replace("SHOW", &s.show)
                    .replace("LINK", &link)
                    .replace("SEASON", &s.season.to_string())
            )
        })
        .collect();

    let previous =
        r#"<a href="javascript:updateMainArticle('/list/trakt/watchlist')">Go Back</a><br>"#;
    let entries = format!(
        r#"{}<table border="0">{}</table>"#,
        previous,
        entries.join("")
    );
    Ok(entries)
}

pub async fn trakt_watched_seasons(
    path: Path<String>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let imdb_url = path.into_inner().into();
    let s = state.clone();
    let show_opt = s.db.handle(ImdbRatingsRequest { imdb_url }).await?;
    let empty = || ("".into(), "".into(), "".into());
    let (imdb_url, show, link) =
        show_opt.map_or_else(empty, |(imdb_url, t)| (imdb_url, t.show, t.link));
    let entries = state.db.handle(ImdbSeasonsRequest { show }).await?;
    let entries = trakt_watched_seasons_worker(&link, &imdb_url, &entries)?;
    form_http_response(entries)
}

pub async fn trakt_watched_list(
    path: Path<(String, i32)>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let (imdb_url, season) = path.into_inner();

    let req = WatchedListRequest {
        imdb_url: imdb_url.into(),
        season,
    };
    let x = state.db.handle(req).await?;
    form_http_response(x)
}

pub async fn trakt_watched_action(
    path: Path<(String, String, i32, i32)>,
    _: LoggedUser,
    state: Data<AppState>,
) -> Result<HttpResponse, Error> {
    let (action, imdb_url, season, episode) = path.into_inner();

    let req = WatchedActionRequest {
        action: action.parse().expect("impossible"),
        imdb_url: imdb_url.into(),
        season,
        episode,
    };
    let x = state.db.handle(req).await?;
    form_http_response(x)
}

fn trakt_cal_worker(entries: &[String]) -> Result<HttpResponse, Error> {
    let previous = r#"<a href="javascript:updateMainArticle('/list/tvshows')">Go Back</a><br>"#;
    let entries = format!(
        r#"{}<table border="0">{}</table>"#,
        previous,
        entries.join("")
    );
    form_http_response(entries)
}

pub async fn trakt_cal(_: LoggedUser, state: Data<AppState>) -> Result<HttpResponse, Error> {
    let req = TraktCalRequest {};
    let entries = state.db.handle(req).await?;
    trakt_cal_worker(&entries)
}

pub async fn user(user: LoggedUser) -> Result<HttpResponse, Error> {
    to_json(user)
}

pub async fn trakt_auth_url(_: LoggedUser, _: Data<AppState>) -> Result<HttpResponse, Error> {
    TRAKT_CONN.init().await;
    let url = TRAKT_CONN.get_auth_url().await?;
    form_http_response(url.to_string())
}

#[derive(Serialize, Deserialize)]
pub struct TraktCallbackRequest {
    pub code: String,
    pub state: String,
}

pub async fn trakt_callback(
    query: Query<TraktCallbackRequest>,
    _: LoggedUser,
    _: Data<AppState>,
) -> Result<HttpResponse, Error> {
    TRAKT_CONN.init().await;
    TRAKT_CONN
        .exchange_code_for_auth_token(query.code.as_str(), query.state.as_str())
        .await?;
    let body = r#"
        <title>Trakt auth code received!</title>
        This window can be closed.
        <script language="JavaScript" type="text/javascript">window.close()</script>"#;
    form_http_response(body.to_string())
}

pub async fn refresh_auth(_: LoggedUser, _: Data<AppState>) -> Result<HttpResponse, Error> {
    TRAKT_CONN.init().await;
    TRAKT_CONN.exchange_refresh_token().await?;
    form_http_response("finished".to_string())
}
