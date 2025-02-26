use crate::{database, git_platform::{GitEventAPI, GitPlatform}};

use std::borrow::BorrowMut;

use chrono::{DateTime, Utc};
use log::{error, log_enabled, trace, Level};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Row, Transaction};
use time::Date;

static GITLAB: OnceCell<Gitlab> = OnceCell::new();

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitlabEvent {
    pub project_id: u64,
    pub action_name: String,
    pub created_at: String,
    pub push_data: Option<PushData>,
}

impl GitEventAPI for GitlabEvent {}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushData {
    pub commit_count: u64,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitlabProjectAPI {
    pub id: u64,
    pub name_with_namespace: String,
    pub web_url: String,
    pub visibility: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitlabProject {
    pub id: u64,
    pub platform_project_id: u64,
    pub name: String,
    pub url: String,
}

#[derive(Debug)]
pub struct Gitlab {
    token: String,
    user_id: String,
}

impl GitPlatform for Gitlab {
    const GIT_PLATFORM_ID: &'static str = "Gitlab";
    type GitEventAPI = GitlabEvent;

    fn init_from_env_vars() -> Self {
        Gitlab {
            token: std::env::var("GITLAB_API_TOKEN")
                .expect("Please specify GITLAB_API_TOKEN as env var!"),
            user_id: std::env::var("GITLAB_USER_ID")
                .expect("Please specify GITLAB_USER_ID as env var!"),
        }
    }

    async fn get_events(&mut self) -> Vec<Self::GitEventAPI> {
        todo!()
    }
}

impl Gitlab {
    pub fn get_or_init() {
        GITLAB.get_or_init(|| Self::init_from_env_vars());
    }

    pub async fn get_events(&self, after: Date, before: Date) -> Vec<GitlabEvent> {
        let client = reqwest::Client::new();
        let token = &self.token;
        let user_id = &self.user_id;
        let url = format!(
            "https://gitlab.com/api/v4/users/{}/events?after={}&before={}",
            user_id,
            after.to_string(),
            before.to_string()
        );

        if after >= before {
            warn!(
                "after: {} >= before: {} - Gitlab API properly won't return any events!",
                after, before
            );
        }
        info!("Getting events from Gitlab... ({})", url);

        let mut gitlab_events: Vec<GitlabEvent> = Vec::new();

        let mut current_page = 1;
        loop {
            let res = client
                .get(format!("{}&page={}", url, current_page))
                .bearer_auth(token)
                .send()
                .await;

            let initial_res = match res {
                Ok(initial_response) => initial_response,
                Err(err) => panic!("Unable to get response from Gitlab!"),
            };

            let status = initial_res.status();
            let header = initial_res.headers().clone();
            let payload = match initial_res.text().await {
                Ok(text) => text,
                Err(err) => panic!("Unable to decode response from Gitlab: {}", err),
            };
            debug!("{:?}", payload);

            if !status.is_success() {
                error!("We got this data: {}", payload.as_str());
                panic!("Couldn't fetch events from Gitlab! {}", status.as_str());
            }

            let total_pages = match header.get("x-total-pages") {
                Some(x_total_pages) => x_total_pages
                    .to_str()
                    .expect("Unable to get string from header")
                    .parse::<u32>()
                    .expect("x-total is not a valid number!"),
                None => panic!("Didn't got x-total header back from Gitlab!"),
            };
            if current_page == 1 && total_pages > 20 {
                warn!("Getting more than 20 pages/400 events! [{}]", total_pages)
            }

            match header.get("x-page") {
                Some(x_page) => {
                    let gitlab_current_page = x_page
                        .to_str()
                        .expect("Unable to get string from header")
                        .parse::<u32>()
                        .expect("x-page is not a valid number!");
                    assert_eq!(gitlab_current_page, current_page);
                }
                None => panic!("Didn't got x-page header back from Gitlab!"),
            }

            let mut data: Vec<GitlabEvent> = match serde_json::from_str(&payload) {
                Ok(data) => data,
                Err(err) => panic!(
                    "Unable to decode json response from Gitlab: {}\nThis is what we received:\n{}",
                    err, payload
                ),
            };

            gitlab_events.append(data.borrow_mut());

            if log_enabled!(Level::Debug) {
                for element in data {
                    debug!("{:?}", element);
                }
                debug!("This was page {} of {}", current_page, total_pages);
            }

            if current_page >= total_pages {
                break;
            }
            current_page += 1;
        }

        gitlab_events
    }

    pub async fn get_project_details_by_id(&self, gitlab_project_id: u64) -> GitlabProjectAPI {
        let client = reqwest::Client::new();
        let token = &self.token;
        let user_id = &self.user_id;
        let url = format!("https://gitlab.com/api/v4/projects/{}", gitlab_project_id);

        info!("Getting project info from Gitlab... ({})", url);

        let res = client.get(url).bearer_auth(token).send().await;

        let initial_res = match res {
            Ok(initial_response) => initial_response,
            Err(err) => panic!("Unable to get response from Gitlab!"),
        };

        let payload = match initial_res.text().await {
            Ok(text) => text,
            Err(err) => panic!("Unable to decode response from Gitlab: {}", err),
        };

        match serde_json::from_str(&payload) {
            Ok(data) => data,
            Err(err) => panic!(
                "Unable to decode json response from Gitlab: {}\nThis is what we received:\n{}",
                err, payload
            ),
        }
    }

    async fn fetch_project_from_gitlab_and_write_to_db(
        &self,
        tx: &mut Transaction<'static, MySql>,
        project_id: u64,
    ) -> u64 {
        let gitlab_project_future = self.get_project_details_by_id(project_id);

        Gitlab::set_platform(tx).await; // TODO: Only do this at initial setup

        let gitlab_project = gitlab_project_future.await;
        let project_id =
            sqlx::query("INSERT INTO GitProjects (platform, platform_project_id, name, url) VALUES ( ?, ?, ?, ? )")
                .bind(Self::GIT_PLATFORM_ID)
                .bind(gitlab_project.id)
                .bind(gitlab_project.name_with_namespace)
                .bind(gitlab_project.web_url)
                .execute(&mut **tx)
                .await
                .unwrap()
                .last_insert_id();
        trace!("Inserted GitProject (Gitlab) id: {}", project_id);
        project_id
    }

    pub async fn insert_gitlab_events_into_db(&self, events: Vec<GitlabEvent>) {
        let db = database::Database::get_or_init().await;
        let pool = db.get_pool().await;

        let tables: Vec<(String,)> = sqlx::query_as("SHOW TABLES")
            .fetch_all(&pool)
            .await
            .unwrap();

        for table in tables.iter() {
            println!("{}", table.0);
            println!("{:?}", table);
        }

        for event in events.iter() {
            // Starting transaction 💪
            let mut tx = pool.begin().await.expect("Couldn't start transaction!");
            let tx_ref = tx.borrow_mut();

            // TODO: Maybe check if name is still up-to-date etc.
            let gitlab_project_option_future =
                Gitlab::fetch_single_git_project_from_db(tx_ref, event.project_id);

            let datetime: DateTime<Utc> = match event.created_at.parse() {
                Ok(datetime) => datetime,
                Err(err) => {
                    // Parsing failed - https://docs.rs/chrono/latest/chrono/struct.DateTime.html#impl-FromStr-for-DateTime%3CUtc%3E
                    error!("Couldn't parse date from Gitlab using a relaxed form of RFC3339. Event will be skipped! Received 'created_at' value: {} - error msg: {}", event.created_at, err);
                    continue;
                }
            };

            // Inserting GitlabProject
            // TODO fetching name + url from gitlab and insert it, if missing
            let project_id = if let Some(project) = gitlab_project_option_future.await {
                project.id
            } else {
                self.fetch_project_from_gitlab_and_write_to_db(tx_ref, event.project_id)
                    .await
            };

            // TODO: Handle push_data (multiple commits!)
            let action_id =
                match Gitlab::get_git_action_by_name(tx_ref, &event.action_name).await {
                    Some(value) => value,
                    None => Gitlab::insert_git_action(tx_ref, &event.action_name).await,
                };

            // Add event itself
            let event_id = Gitlab::insert_event(tx_ref, datetime).await;

            let gitlab_event_id =
                Gitlab::insert_git_event(&mut tx, event_id, action_id, project_id).await;

            // let event_id = sqlx::query("INSERT INTO GitlabProjects (id, name, url) VALUES ( ? )")
            //     .bind(event.)
            //     .execute(&mut *tx)
            //     .await
            //     .unwrap()
            //     .last_insert_id();
            // trace!("Inserted Gitlab event id: {} @ {}", event_id, datetime);

            tx.commit().await.expect("Couldn't apply transaction ._.");
        }

        assert!(tables.len() > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotenv::dotenv;

    #[tokio::test]
    async fn gitlab_api_is_still_sane() {
        dotenv().ok();
        let gitlab = Gitlab::init_from_env_vars();

        let result = gitlab
            .get_events(
                time::macros::date!(2024 - 05 - 01),
                time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
            )
            .await;
        //assert_eq!(result, OffsetDateTime::now_utc().date().to_string())
        assert_eq!(result.len(), 31);
    }

    #[tokio::test]
    async fn gitlab_api_is_still_sane_without_pagination() {
        dotenv().ok();
        Gitlab::get_or_init();
        let gitlab = Gitlab::init_from_env_vars();

        let result = gitlab
            .get_events(
                time::macros::date!(2024 - 05 - 03),
                time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
            )
            .await;
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn gitlab_get_pollux_project() {
        dotenv().ok();
        Gitlab::get_or_init();
        let gitlab = Gitlab::init_from_env_vars();

        let result = gitlab.get_project_details_by_id(61345567).await;
        println!("{:?}", result);
        assert_eq!(
            result,
            GitlabProjectAPI {
                id: 61345567,
                name_with_namespace: "2tefan Projects / Stats / Pollux".to_string(),
                web_url: "https://gitlab.com/2tefan-projects/stats/pollux".to_string(),
                visibility: Some("public".to_string())
            }
        );
    }

    #[tokio::test]
    async fn import_data_from_gitlab_into_database() {
        dotenv().ok();
        Gitlab::get_or_init();
        let gitlab = Gitlab::init_from_env_vars();

        let events = gitlab
            .get_events(
                time::macros::date!(2024 - 05 - 03),
                time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
            )
            .await;
        gitlab.insert_gitlab_events_into_db(events).await; // TODO: Fix test
    }
}
