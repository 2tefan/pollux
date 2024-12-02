use crate::database;

use std::borrow::{Borrow, BorrowMut};

use chrono::{DateTime, Utc};
use log::{error, log_enabled, trace, Level};
use once_cell::sync::OnceCell;
use rocket::futures::TryStreamExt;
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

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushData {
    pub commit_count: u64,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitlabProject {
    pub id: u64,
    pub name_with_namespace: String,
    pub web_url: String,
    pub visibility: Option<String>,
}

#[derive(Debug)]
pub struct Gitlab {
    token: String,
    user_id: String,
}

impl Gitlab {
    pub fn init_from_env_vars() -> Gitlab {
        Gitlab {
            token: std::env::var("GITLAB_API_TOKEN")
                .expect("Please specify GITLAB_API_TOKEN as env var!"),
            user_id: std::env::var("GITLAB_USER_ID")
                .expect("Please specify GITLAB_USER_ID as env var!"),
        }
    }

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

            let header = initial_res.headers().clone();
            let payload = match initial_res.text().await {
                Ok(text) => text,
                Err(err) => panic!("Unable to decode response from Gitlab: {}", err),
            };

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

    pub async fn get_project_details_by_id(&self, gitlab_project_id: u64) -> GitlabProject {
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

    pub async fn get_gitlab_action_by_name(
        tx: &mut Transaction<'static, MySql>,
        action_name: &String,
    ) -> Option<u64> {
        let mut rows = sqlx::query("SELECT id FROM GitActions WHERE name = ?")
            .bind(action_name)
            .fetch(&mut **tx);

        let mut number_of_actions = 0;
        let mut gitlab_action_id = Option::None;
        while let Some(row) = rows.try_next().await.unwrap() {
            if number_of_actions > 0 {
                error!(
                    "There are more than 1x Gitlab Actions with the same name! (name={}) - skipping this event!",
                    action_name
                );
                return Option::None;
            }

            number_of_actions += 1;
            gitlab_action_id = Some(row.try_get("id").unwrap());
        }

        gitlab_action_id
    }

    pub async fn fetch_single_gitlab_project_from_db(
        tx: &mut Transaction<'static, MySql>,
        project_id: u64,
    ) -> Option<GitlabProject> {
        let mut rows =
            sqlx::query("SELECT gitlab_id, name, url FROM GitlabProjects WHERE gitlab_id = ?")
                .bind(project_id)
                .fetch(&mut **tx);

        let mut number_of_projects = 0;
        let mut gitlab_project = Option::None;
        while let Some(row) = rows.try_next().await.unwrap() {
            if number_of_projects > 0 {
                error!(
                    "There are more than 1x Gitlab projects in DB (id={}) - skipping this event!",
                    project_id
                );
                return Option::None;
            }

            number_of_projects += 1;
            let gitlab_id: u64 = row.try_get("gitlab_id").unwrap();
            let name: &str = row.try_get("name").unwrap();
            let url: &str = row.try_get("url").unwrap();
            gitlab_project = Some(GitlabProject {
                id: gitlab_id,
                name_with_namespace: name.to_string(),
                web_url: url.to_string(),
                visibility: Option::None,
            });
        }

        gitlab_project
    }

    async fn fetch_project_from_gitlab_and_write_to_db(
        &self,
        tx: &mut Transaction<'static, MySql>,
        project_id: u64,
    ) {
        let gitlab_project = self.get_project_details_by_id(project_id).await;
        let project_id =
            sqlx::query("INSERT INTO GitlabProjects (gitlab_id, name, url) VALUES ( ?, ?, ? )")
                .bind(gitlab_project.id)
                .bind(gitlab_project.name_with_namespace)
                .bind(gitlab_project.web_url)
                .execute(&mut **tx)
                .await
                .unwrap()
                .last_insert_id();
        trace!("Inserted GitlabProject id: {}", project_id);
    }

    async fn insert_gitlab_action(
        tx: &mut Transaction<'static, MySql>,
        action_name: &String,
    ) -> u64 {
        let action_id = sqlx::query("INSERT INTO GitActions (name) VALUES ( ? )")
            .bind(action_name)
            .execute(&mut **tx)
            .await
            .unwrap()
            .last_insert_id();
        trace!("Inserted Gitlab action id: {} ({})", action_id, action_name);
        return action_id;
    }

    async fn insert_event(tx: &mut Transaction<'static, MySql>, datetime: DateTime<Utc>) -> u64 {
        let event_id = sqlx::query("INSERT INTO Events (timestamp) VALUES ( ? )")
            .bind(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
            .execute(&mut **tx)
            .await
            .unwrap()
            .last_insert_id();
        trace!("Inserted Gitlab event id: {} @ {}", event_id, datetime);
        return event_id;
    }

    async fn insert_gitlab_event(
        tx: &mut Transaction<'static, MySql>,
        event_id: u64,
        action_id: u64,
        project_id: u64,
    ) -> u64 {
        sqlx::query("INSERT INTO GitlabEvents (id, action_id, gitlab_project_id) VALUES ( ?, ?, ?)")
            .bind(event_id)
            .bind(action_id)
            .bind(project_id)
            .execute(&mut **tx)
            .await
            .unwrap()
            .last_insert_id()
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
            let gitlab_project_option =
                Gitlab::fetch_single_gitlab_project_from_db(tx_ref, event.project_id);

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
            if gitlab_project_option.await.is_none() {
                self.fetch_project_from_gitlab_and_write_to_db(tx_ref, event.project_id)
                    .await;
            }

            // TODO: Handle push_data (multiple commits!)
            let action_id =
                match Gitlab::get_gitlab_action_by_name(tx_ref, &event.action_name).await {
                    Some(value) => value,
                    None => Gitlab::insert_gitlab_action(tx_ref, &event.action_name).await,
                };

            // Add event itself
            let event_id = Gitlab::insert_event(tx_ref, datetime).await;

            let gitlab_event_id = Gitlab::insert_gitlab_event(&mut tx, event_id, action_id, event.project_id).await;

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
            GitlabProject {
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
