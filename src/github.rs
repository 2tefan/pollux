use crate::database;

use std::borrow::{Borrow, BorrowMut};

use chrono::{DateTime, Utc};
use log::{error, log_enabled, trace, Level};
use once_cell::sync::OnceCell;
use reqwest::{
    header::{self, HeaderMap, HeaderValue, ACCEPT, ETAG, IF_NONE_MATCH, USER_AGENT},
    StatusCode,
};
use rocket::futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Row, Transaction};
use time::Date;

static GITHUB: OnceCell<Github> = OnceCell::new();
static GIT_PLATFORM_ID: &str = "Github";

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GithubEvent {
    pub created_at: String,
    pub public: bool,
    #[serde(rename = "type")]
    pub type_of_action: String,
    pub repo: GithubRepoAPI,
    // action maybe?
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GithubRepoAPI {
    pub id: u64,
    pub name: String,
    pub url: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GithubProjectAPI {
    pub id: u64,
    pub name_with_namespace: String,
    pub web_url: String,
    pub visibility: Option<String>,
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

impl Github {
    pub fn init_from_env_vars() -> Github {
        Github {
            token: std::env::var("GITHUB_API_TOKEN")
                .expect("Please specify GITHUB_API_TOKEN as env var!"),
            username: std::env::var("GITHUB_USERNAME")
                .expect("Please specify GITHUB_USERNAME as env var!"),
            e_tag: Vec::new(), // Maybe save tag in DB and fetch it again on startup?
        }
    }

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

    pub async fn get_events(&mut self) -> Vec<GithubEvent> {
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

    // // TODO
    // pub async fn get_project_details_by_id(&self, github_project_id: u64) -> GithubProjectAPI {
    //     let client = reqwest::Client::new();
    //     let token = &self.token;
    //     let user_id = &self.username;
    //     let url = format!("https://github.com/api/v4/projects/{}", github_project_id);

    //     info!("Getting project info from Github... ({})", url);

    //     let res = client.get(url).bearer_auth(token).send().await;

    //     let initial_res = match res {
    //         Ok(initial_response) => initial_response,
    //         Err(err) => panic!("Unable to get response from Github!"),
    //     };

    //     let payload = match initial_res.text().await {
    //         Ok(text) => text,
    //         Err(err) => panic!("Unable to decode response from Github: {}", err),
    //     };

    //     match serde_json::from_str(&payload) {
    //         Ok(data) => data,
    //         Err(err) => panic!(
    //             "Unable to decode json response from Github: {}\nThis is what we received:\n{}",
    //             err, payload
    //         ),
    //     }
    // }

    // // TODO
    // pub async fn set_platform(tx: &mut Transaction<'static, MySql>) {
    //     let rows = sqlx::query("SELECT name FROM GitPlatforms WHERE name = ?")
    //         .bind(GIT_PLATFORM_ID)
    //         .fetch_all(&mut **tx) // Use fetch_all to collect all rows immediately
    //         .await
    //         .unwrap();

    //     if rows.len() > 1 {
    //         panic!(
    //         "There are more than 1x Github Platforms with the same name! (name={}) - This can't be!",
    //         GIT_PLATFORM_ID
    //     );
    //     }

    //     // Add platform, if it not yet exists
    //     if rows.is_empty() {
    //         sqlx::query("INSERT INTO GitPlatforms (name) VALUES ( ? )")
    //             .bind(GIT_PLATFORM_ID)
    //             .execute(&mut **tx)
    //             .await
    //             .unwrap();
    //     }
    // }

    // // TODO
    // pub async fn get_github_action_by_name(
    //     tx: &mut Transaction<'static, MySql>,
    //     action_name: &String,
    // ) -> Option<u64> {
    //     let mut rows = sqlx::query("SELECT id FROM GitActions WHERE name = ?")
    //         .bind(action_name)
    //         .fetch(&mut **tx);

    //     let mut number_of_actions = 0;
    //     let mut github_action_id = Option::None;
    //     while let Some(row) = rows.try_next().await.unwrap() {
    //         if number_of_actions > 0 {
    //             error!(
    //                 "There are more than 1x Github Actions with the same name! (name={}) - skipping this event!",
    //                 action_name
    //             );
    //             return Option::None;
    //         }

    //         number_of_actions += 1;
    //         github_action_id = Some(row.try_get("id").unwrap());
    //     }

    //     github_action_id
    // }

    // // TODO
    // pub async fn fetch_single_github_project_from_db(
    //     tx: &mut Transaction<'static, MySql>,
    //     platform_project_id: u64,
    // ) -> Option<GithubProject> {
    //     let mut rows =
    //         sqlx::query("SELECT id, platform_project_id, name, url FROM GitProjects WHERE platform_project_id = ? AND platform = ?")
    //             .bind(platform_project_id)
    //             .bind(GIT_PLATFORM_ID)
    //             .fetch(&mut **tx);

    //     let mut number_of_projects = 0;
    //     let mut github_project = Option::None;
    //     while let Some(row) = rows.try_next().await.unwrap() {
    //         if number_of_projects > 0 {
    //             error!(
    //                 "There are more than 1x Github projects in DB (id={}) - skipping this event!",
    //                 platform_project_id
    //             );
    //             return Option::None;
    //         }

    //         number_of_projects += 1;
    //         let id: u64 = row.try_get("id").unwrap();
    //         let platform_project_id: u64 = row.try_get("platform_project_id").unwrap();
    //         let name: &str = row.try_get("name").unwrap();
    //         let url: &str = row.try_get("url").unwrap();
    //         github_project = Some(GithubProject {
    //             id,
    //             platform_project_id,
    //             name: name.to_string(),
    //             url: url.to_string(),
    //         });
    //     }

    //     github_project
    // }

    // // TODO
    // async fn fetch_project_from_github_and_write_to_db(
    //     &self,
    //     tx: &mut Transaction<'static, MySql>,
    //     project_id: u64,
    // ) -> u64 {
    //     let github_project_future = self.get_project_details_by_id(project_id);

    //     Github::set_platform(tx).await; // TODO: Only do this at initial setup

    //     let github_project = github_project_future.await;
    //     let project_id =
    //         sqlx::query("INSERT INTO GitProjects (platform, platform_project_id, name, url) VALUES ( ?, ?, ?, ? )")
    //             .bind(GIT_PLATFORM_ID)
    //             .bind(github_project.id)
    //             .bind(github_project.name_with_namespace)
    //             .bind(github_project.web_url)
    //             .execute(&mut **tx)
    //             .await
    //             .unwrap()
    //             .last_insert_id();
    //     trace!("Inserted GitProject (Github) id: {}", project_id);
    //     project_id
    // }

    // // TODO
    // async fn insert_github_action(
    //     tx: &mut Transaction<'static, MySql>,
    //     action_name: &String,
    // ) -> u64 {
    //     let action_id = sqlx::query("INSERT INTO GitActions (name) VALUES ( ? )")
    //         .bind(action_name)
    //         .execute(&mut **tx)
    //         .await
    //         .unwrap()
    //         .last_insert_id();
    //     trace!("Inserted Github action id: {} ({})", action_id, action_name);
    //     return action_id;
    // }

    // // TODO
    // async fn insert_event(tx: &mut Transaction<'static, MySql>, datetime: DateTime<Utc>) -> u64 {
    //     let event_id = sqlx::query("INSERT INTO Events (timestamp) VALUES ( ? )")
    //         .bind(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
    //         .execute(&mut **tx)
    //         .await
    //         .unwrap()
    //         .last_insert_id();
    //     trace!("Inserted Github event id: {} @ {}", event_id, datetime);
    //     return event_id;
    // }

    // // TODO
    // async fn insert_github_event(
    //     tx: &mut Transaction<'static, MySql>,
    //     event_id: u64,
    //     action_id: u64,
    //     project_id: u64,
    // ) -> u64 {
    //     sqlx::query("INSERT INTO GitEvents (id, action_fk, project_fk) VALUES ( ?, ?, ? )")
    //         .bind(event_id)
    //         .bind(action_id)
    //         .bind(project_id)
    //         .execute(&mut **tx)
    //         .await
    //         .unwrap()
    //         .last_insert_id()
    // }

    // // TODO
    // pub async fn insert_github_events_into_db(&self, events: Vec<GithubEvent>) {
    //     let db = database::Database::get_or_init().await;
    //     let pool = db.get_pool().await;

    //     let tables: Vec<(String,)> = sqlx::query_as("SHOW TABLES")
    //         .fetch_all(&pool)
    //         .await
    //         .unwrap();

    //     for table in tables.iter() {
    //         println!("{}", table.0);
    //         println!("{:?}", table);
    //     }

    //     for event in events.iter() {
    //         // Starting transaction 💪
    //         let mut tx = pool.begin().await.expect("Couldn't start transaction!");
    //         let tx_ref = tx.borrow_mut();

    //         // TODO: Maybe check if name is still up-to-date etc.
    //         let github_project_option_future =
    //             Github::fetch_single_github_project_from_db(tx_ref, event.project_id);

    //         let datetime: DateTime<Utc> = match event.created_at.parse() {
    //             Ok(datetime) => datetime,
    //             Err(err) => {
    //                 // Parsing failed - https://docs.rs/chrono/latest/chrono/struct.DateTime.html#impl-FromStr-for-DateTime%3CUtc%3E
    //                 error!("Couldn't parse date from Github using a relaxed form of RFC3339. Event will be skipped! Received 'created_at' value: {} - error msg: {}", event.created_at, err);
    //                 continue;
    //             }
    //         };

    //         // Inserting GithubProject
    //         // TODO fetching name + url from github and insert it, if missing
    //         let project_id = if let Some(project) = github_project_option_future.await {
    //             project.id
    //         } else {
    //             self.fetch_project_from_github_and_write_to_db(tx_ref, event.project_id)
    //                 .await
    //         };

    //         // TODO: Handle push_data (multiple commits!)
    //         let action_id =
    //             match Github::get_github_action_by_name(tx_ref, &event.action_name).await {
    //                 Some(value) => value,
    //                 None => Github::insert_github_action(tx_ref, &event.action_name).await,
    //             };

    //         // Add event itself
    //         let event_id = Github::insert_event(tx_ref, datetime).await;

    //         let github_event_id =
    //             Github::insert_github_event(&mut tx, event_id, action_id, project_id).await;

    //         // let event_id = sqlx::query("INSERT INTO GithubProjects (id, name, url) VALUES ( ? )")
    //         //     .bind(event.)
    //         //     .execute(&mut *tx)
    //         //     .await
    //         //     .unwrap()
    //         //     .last_insert_id();
    //         // trace!("Inserted Github event id: {} @ {}", event_id, datetime);

    //         tx.commit().await.expect("Couldn't apply transaction ._.");
    //     }

    //     assert!(tables.len() > 0);
    // }
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

    // #[tokio::test]
    // async fn github_api_is_still_sane_without_pagination() {
    //     dotenv().ok();
    //     Github::get_or_init();
    //     let github = Github::init_from_env_vars();

    //     let result = github
    //         .get_events(
    //             time::macros::date!(2024 - 05 - 03),
    //             time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
    //         )
    //         .await;
    //     assert_eq!(result.len(), 4);
    // }

    // #[tokio::test]
    // async fn github_get_pollux_project() {
    //     dotenv().ok();
    //     Github::get_or_init();
    //     let github = Github::init_from_env_vars();

    //     let result = github.get_project_details_by_id(61345567).await;
    //     println!("{:?}", result);
    //     assert_eq!(
    //         result,
    //         GithubProjectAPI {
    //             id: 61345567,
    //             name_with_namespace: "2tefan Projects / Stats / Pollux".to_string(),
    //             web_url: "https://github.com/2tefan-projects/stats/pollux".to_string(),
    //             visibility: Some("public".to_string())
    //         }
    //     );
    // }

    // #[tokio::test]
    // async fn import_data_from_github_into_database() {
    //     dotenv().ok();
    //     Github::get_or_init();
    //     let github = Github::init_from_env_vars();

    //     let events = github
    //         .get_events(
    //             time::macros::date!(2024 - 05 - 03),
    //             time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
    //         )
    //         .await;
    //     github.insert_github_events_into_db(events).await; // TODO: Fix test
    // }
}
