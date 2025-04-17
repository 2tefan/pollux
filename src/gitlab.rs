use crate::{
    database,
    git_platform::{GitEventAPI, GitPlatform},
};

use std::{borrow::BorrowMut, sync::Arc};

use chrono::{DateTime, Utc};
use log::{error, log_enabled, trace, warn, Level};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Transaction};
use tokio::sync::Mutex;

static GITLAB: OnceCell<Arc<Mutex<Gitlab>>> = OnceCell::new();

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
        let before = match Gitlab::get_last_sync_timestamp().await {
            Some(value) => value,
            None => {
                info!("Initial run! Fetching last 90 days from Gitlab...");
                Utc::now() - chrono::Duration::days(90)
            }};
        Gitlab::get_events(
            &self, 
            before,
            Utc::now()
        ).await
    }

    async fn update_provider(&mut self) -> Option<i32> {
        info!("Updating events from Gitlab...");
        let events = self.get_events().await;
        let new_events = self.insert_gitlab_events_into_db(events).await;

        Some(new_events)
    }
}

impl Gitlab {
    pub fn get_or_init() -> Arc<Mutex<Gitlab>>{
        GITLAB.get_or_init(|| Arc::new(Mutex::new(Self::init_from_env_vars()))).clone()
    }

    pub async fn get_events(&self, after: DateTime<Utc>, before: DateTime<Utc>) -> Vec<GitlabEvent> {
        let client = reqwest::Client::new();
        let token = &self.token;
        let user_id = &self.user_id;
        let url = format!(
            "https://gitlab.com/api/v4/users/{}/events?after={}&before={}",
            user_id,
            after.format("%Y-%m-%d").to_string(),
            before.format("%Y-%m-%d").to_string()
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
                Err(err) => panic!("Unable to get response from Gitlab!: {}", err),
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
                warn!(
                    "Getting more than 20 pages/400 events! [{} pages]",
                    total_pages
                )
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
        let url = format!("https://gitlab.com/api/v4/projects/{}", gitlab_project_id);

        info!("Getting project info from Gitlab... ({})", url);

        let res = client.get(url).bearer_auth(token).send().await;

        let initial_res = match res {
            Ok(initial_response) => initial_response,
            Err(err) => panic!("Unable to get response from Gitlab! {}", err),
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
    ) -> Result<u64, String> {
        let gitlab_project_future = self.get_project_details_by_id(project_id);

        Gitlab::set_platform(tx).await; // TODO: Only do this at initial setup

        let gitlab_project = gitlab_project_future.await;

        if gitlab_project.visibility.unwrap() != "public" {
            return Err("Skipping not public project".to_string());
        }

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
        Ok(project_id)
    }

    pub async fn insert_gitlab_events_into_db(&self, events: Vec<GitlabEvent>) -> i32 {
        let db = database::Database::get_or_init().await;
        let pool = db.get_pool().await;

        info!("Starting to insert events from Gitlab");
        let mut total_events = 0;
        let mut added_events = 0;

        // Starting transaction 💪
        let mut tx = pool.begin().await.expect("Couldn't start transaction!");
        let tx_ref = tx.borrow_mut();
        Self::set_platform(tx_ref).await; // TODO: Only do this at initial setup

        for event in events.iter() {
            total_events += 1;

            // TODO: Maybe check if name is still up-to-date etc.
            let gitlab_project_option_future =
                Gitlab::fetch_single_git_project_from_db(tx_ref, event.project_id);

            let datetime: DateTime<Utc> = match event.created_at.parse() {
                Ok(datetime) => datetime,
                Err(err) => {
                    // Parsing failed - https://docs.rs/chrono/latest/chrono/struct.DateTime.html#impl-FromStr-for-DateTime%3CUtc%3E
                    error!(
                        "Couldn't parse date from Gitlab using a relaxed form of RFC3339. \
                    Event will be skipped! Received 'created_at' value: {} - error msg: {}",
                    event.created_at, err
                    );
                    continue;
                }
            };

            // Inserting GitlabProject
            let project_id = if let Some(project) = gitlab_project_option_future.await {
                project.id
            } else {
                match self.fetch_project_from_gitlab_and_write_to_db(tx_ref, event.project_id)
                    .await {
                        Ok(result) => result,
                        Err(err) => {
                            debug!("Skipping event: {}", err);
                            continue;
                        }
                    }
            };

            let action_name = match Gitlab::map_action_name(event.action_name.as_str()) {
                Some(value) => value,
                None => {
                    warn!("Skipping event - because action name unknown! {:#?}", event);
                    continue;
                }
            };
            // TODO: Handle push_data (multiple commits!)
            let action_id = match Gitlab::get_git_action_by_name(tx_ref, &action_name).await {
                Some(value) => value,
                None => Gitlab::insert_git_action(tx_ref, &action_name).await,
            };

            if Gitlab::count_all_matching_events(tx_ref, &datetime, &action_id, &project_id).await
                > 0
            {
                debug!("Skipping insert! Event already exists");
                continue;
            }

            // Add event itself
            let event_id = Gitlab::insert_event(tx_ref, datetime).await;

            let _gitlab_event_id =
                Gitlab::insert_git_event(tx_ref, event_id, action_id, project_id).await;

            // let event_id = sqlx::query("INSERT INTO GitlabProjects (id, name, url) VALUES ( ? )")
            //     .bind(event.)
            //     .execute(&mut *tx)
            //     .await
            //     .unwrap()
            //     .last_insert_id();
            // trace!("Inserted Gitlab event id: {} @ {}", event_id, datetime);

            added_events += 1;
        }

        Gitlab::update_last_sync_timestamp(tx_ref).await;
        tx.commit().await.expect("Couldn't apply transaction ._.");
        info!(
            "Inserted {} new Gitlab events from {} total events into DB",
            added_events, total_events
        );

        added_events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use dotenv::dotenv;

    #[tokio::test]
    async fn gitlab_api_is_still_sane() {
        dotenv().ok();
        let gitlab = Gitlab::init_from_env_vars();

        let result = gitlab
            .get_events(
                Utc.with_ymd_and_hms(2024, 05, 01, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2024, 05, 05, 0, 0, 0).unwrap(),
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
                Utc.with_ymd_and_hms(2024, 05, 03, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2024, 05, 05, 0, 0, 0).unwrap(),
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
                Utc.with_ymd_and_hms(2024, 05, 03, 0, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2024, 05, 05, 0, 0, 0).unwrap(),
            )
            .await;
        gitlab.insert_gitlab_events_into_db(events).await; // TODO: Fix test
    }
}
