use crate::{
    database,
    git_platform::{GitEventAPI, GitPlatform, GitProject},
};

use std::borrow::BorrowMut;

use chrono::{DateTime, Utc};
use log::{error, log_enabled, Level};
use once_cell::sync::OnceCell;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, IF_NONE_MATCH, USER_AGENT},
    StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

static GITHUB: OnceCell<Github> = OnceCell::new();
static GIT_PLATFORM_ID: &str = "Github";

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GithubEvent {
    pub created_at: String,
    pub public: bool,
    #[serde(rename = "type")]
    pub type_of_action: String,
    pub repo: GithubProjectAPI,
    // action maybe?
}

impl GitEventAPI for GithubEvent {}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GithubProjectAPI {
    pub id: u64,
    pub name: String,
    pub url: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GithubProject {
    pub id: u64,
    pub platform_project_id: u64,
    pub name: String,
    pub url: String,
}

#[derive(Debug)]
pub struct Github {
    token: String,
    username: String,
    e_tag: Vec<HeaderValue>,
}

impl GitPlatform for Github {
    const GIT_PLATFORM_ID: &'static str = "Github";
    type GitEventAPI = GithubEvent;

    fn init_from_env_vars() -> Self {
        Github {
            token: std::env::var("GITHUB_API_TOKEN")
                .expect("Please specify GITHUB_API_TOKEN as env var!"),
            username: std::env::var("GITHUB_USERNAME")
                .expect("Please specify GITHUB_USERNAME as env var!"),
            e_tag: Vec::new(), // Maybe save tag in DB and fetch it again on startup?
        }
    }

    async fn get_events(&mut self) -> Vec<Self::GitEventAPI> {
        let client = reqwest::Client::new();
        let token = &self.token;
        let github_username = &self.username;
        let url = format!("https://api.github.com/users/{}/events", github_username);

        info!("Getting events from Github... ({})", url);

        let mut github_events: Vec<GithubEvent> = Vec::new();
        let mut current_page = 1;
        let events_per_page_parameter = 5;
        let mut next_page_url = Some(format!(
            "{}?per_page={}&page={}",
            url, events_per_page_parameter, current_page
        ));

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, "application/vnd.github+json".parse().unwrap());
        headers.insert(USER_AGENT, "2tefan-pollux".parse().unwrap());
        headers.insert("X-GitHub-Api-Version", "2022-11-28".parse().unwrap());

        loop {
            let mut using_etag = false;
            if self.e_tag.get(current_page - 1).is_some() {
                headers.insert(
                    IF_NONE_MATCH,
                    self.e_tag.get(current_page - 1).unwrap().clone(),
                );
                using_etag = true;
            }

            let res = client
                .get(next_page_url.unwrap())
                .bearer_auth(token)
                .headers(headers.clone())
                .send()
                .await;

            let initial_res = match res {
                Ok(initial_response) => initial_response,
                Err(err) => panic!("Unable to get response from Github! ({})", err),
            };

            let status = initial_res.status();
            let header = initial_res.headers().clone();
            let payload = match initial_res.text().await {
                Ok(text) => text,
                Err(err) => panic!("Unable to decode response from Github: {}", err),
            };
            debug!("{:?}", payload);

            if status == StatusCode::NOT_MODIFIED && using_etag {
                debug!("Got 304 from Github + etag/IF_NONE_MATCH was set, so no new events!");
                return github_events;
            }

            if !status.is_success() {
                error!("We got this data: {}", payload.as_str());
                panic!("Couldn't fetch events from Github! {}", status.as_str());
            }

            next_page_url = match header.get("link") {
                Some(link) => Github::parse_header_for_next_page(
                    link.to_str()
                        .expect("Unable to get string from header")
                        .parse()
                        .expect("Couldn't parse link header from Github response!"),
                ),
                None => panic!("Didn't got link header back from Github!"),
            };

            if let Some(etag) = header.get("etag") {
                //headers.append(IF_NONE_MATCH, etag.clone());
                if self.e_tag.len() < current_page {
                    self.e_tag.resize(current_page, etag.clone());
                }
                self.e_tag[current_page - 1] = etag.clone();
            }

            let mut data: Vec<GithubEvent> = match serde_json::from_str(&payload) {
                Ok(data) => data,
                Err(err) => panic!(
                    "Unable to decode json response from Github: {}\nThis is what we received:\n{}",
                    err, payload
                ),
            };

            github_events.append(data.borrow_mut());

            if log_enabled!(Level::Debug) {
                for element in data {
                    debug!("{:?}", element);
                }
            }

            if next_page_url.is_none() {
                debug!("This the last page {}", current_page);
                return github_events;
            }

            debug!(
                "This was page {} - next page is at '{}'",
                current_page,
                next_page_url.clone().unwrap()
            );
            current_page += 1;
        }
    }
}

impl Github {
    pub fn get_or_init() {
        GITHUB.get_or_init(|| Self::init_from_env_vars());
    }

    // Parse header like:
    // < link: <https://api.github.com/user/26086452/events?per_page=2&page=2>; rel="next", <https://api.github.com/user/26086452/events?per_page=2&page=6>; rel="last"
    fn parse_header_for_next_page(header: String) -> Option<String> {
        for link in header.split(",") {
            let parts: Vec<&str> = link.split(";").collect();

            if parts.len() != 2 {
                continue;
            }

            let rel_part = parts[1].trim();
            if rel_part != r#"rel="next""# {
                continue;
            }

            let url_part = parts[0].trim();
            if !(url_part.starts_with("<") && url_part.ends_with(">")) {
                continue;
            }

            return Some(url_part[1..url_part.len() - 1].to_string());
        }

        None
    }

    // // TODO
    pub async fn insert_github_events_into_db(&self, events: Vec<GithubEvent>) {
        let db = database::Database::get_or_init().await;
        let pool = db.get_pool().await;

        for event in events.iter() {
            // Starting transaction 💪
            let mut tx = pool.begin().await.expect("Couldn't start transaction!");
            let tx_ref = tx.borrow_mut();

            // TODO: Maybe check if name is still up-to-date etc.
            let github_project_option_future =
                Github::fetch_single_git_project_from_db(tx_ref, event.repo.id);

            let datetime: DateTime<Utc> = match event.created_at.parse() {
                Ok(datetime) => datetime,
                Err(err) => {
                    // Parsing failed - https://docs.rs/chrono/latest/chrono/struct.DateTime.html#impl-FromStr-for-DateTime%3CUtc%3E
                    error!("Couldn't parse date from Github using a relaxed form of RFC3339. Event will be skipped! Received 'created_at' value: {} - error msg: {}", event.created_at, err);
                    continue;
                }
            };

            // Inserting GithubProject
            // TODO fetching name + url from github and insert it, if missing
            let project_id = if let Some(project) = github_project_option_future.await {
                project.id
            } else {
                self.write_project_to_db(
                    tx_ref,
                    &GitProject {
                        id: event.repo.id, // This is kinda cheating... Pls fix
                        platform_project_id: event.repo.id,
                        name: event.repo.name.clone(),
                        url: event.repo.url.clone(),
                    },
                )
                .await
            };

            // TODO: Handle push_data (multiple commits!)
            let action_id =
                match Github::get_git_action_by_name(tx_ref, &event.type_of_action).await {
                    Some(value) => value,
                    None => Github::insert_git_action(tx_ref, &event.type_of_action).await,
                };

            // Add event itself
            let event_id = Github::insert_event(tx_ref, datetime).await;

            let github_event_id =
                Github::insert_git_event(&mut tx, event_id, action_id, project_id).await;

            tx.commit().await.expect("Couldn't apply transaction ._.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotenv::dotenv;

    #[tokio::test]
    async fn github_api_is_still_sane() {
        dotenv().ok();
        let mut github = Github::init_from_env_vars();

        let result = github.get_events().await;
        //assert_eq!(result, OffsetDateTime::now_utc().date().to_string())
        assert_eq!(result.len(), 12);
    }

    #[tokio::test]
    async fn github_api_is_still_sane_using_etag() {
        dotenv().ok();
        let mut github = Github::init_from_env_vars();

        let result = github.get_events().await;
        let result_not_modified = github.get_events().await;
        //assert_eq!(result, OffsetDateTime::now_utc().date().to_string())
        assert_eq!(result.len(), 12);
        assert_eq!(result_not_modified.len(), 0);
    }

    #[tokio::test]
    async fn import_data_from_github_into_database() {
        dotenv().ok();
        let mut github = Github::init_from_env_vars();

        let events = github.get_events().await;
        github.insert_github_events_into_db(events).await;
    }
}
