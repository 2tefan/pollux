use crate::database;

use std::borrow::{Borrow, BorrowMut};

use log::{log_enabled, Level};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use time::Date;

static GITLAB: OnceCell<Gitlab> = OnceCell::new();

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitlabEvent {
    pub project_id: i64,
    pub action_name: String,
    pub created_at: String,
    pub push_data: Option<PushData>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushData {
    pub commit_count: i64,
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
        println!("{:?}", result);
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn test_todo() {
        // Do something usefull
        dotenv().ok();
        Gitlab::get_or_init();
        let gitlab = Gitlab::init_from_env_vars();

        let events = gitlab
            .get_events(
                time::macros::date!(2024 - 05 - 03),
                time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
            )
            .await;
        gitlab.insert_gitlab_events_into_db(events).await;
    }
}
