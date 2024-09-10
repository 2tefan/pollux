#[macro_use]
extern crate dotenv;
#[macro_use]
extern crate rocket;
#[macro_use]
extern crate serde_derive;
extern crate log;
extern crate tokio;

#[macro_use]
extern crate dotenv_codegen;

use core::{panic, str};
use std::borrow::{Borrow, BorrowMut};
use std::iter;

use dotenv::dotenv;
use env_logger::init;
use once_cell::sync::OnceCell;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rocket::data;
use rocket::futures::stream::iter;
use rocket::http::{ContentType, Status};
use time::{Date, Duration, OffsetDateTime};

extern crate cronjob;
use cronjob::CronJob;

use log::{debug, error, info, log_enabled, warn, Level};
use serde_derive::Deserialize;
use serde_derive::Serialize;
use serde_json::Value;

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
    pub fn global() -> &'static Gitlab {
        GITLAB.get().expect("Gitlab not initialized yet!!")
    }

    fn from_env_vars() -> Gitlab {
        Gitlab {
            token: std::env::var("GITLAB_API_TOKEN")
                .expect("Please specify GITLAB_API_TOKEN as env var!"),
            user_id: std::env::var("GITLAB_USER_ID")
                .expect("Please specify GITLAB_USER_ID as env var!"),
        }
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
}

static GITLAB: OnceCell<Gitlab> = OnceCell::new();

#[get("/")]
async fn index() -> (Status, (ContentType, String)) {
    //unsafe { format!("Hello, world! {}, {}", MY_COUNTER.unwrap_or(0)) }
    (
        Status::ImATeapot,
        (
            ContentType::Text,
            "Hello".to_string(), // Gitlab::global()
                                 //     .get_events(
                                 //         (OffsetDateTime::now_utc() + Duration::days(-2)).date(),
                                 //         (OffsetDateTime::now_utc() + Duration::days(0)).date(),
                                 //     )
                                 //     .await,
        ),
    )
}

#[launch]
fn rocket() -> _ {
    dotenv().ok();
    env_logger::init();

    // Setup cronjobs
    let mut cron = CronJob::new("Test Cron", on_cron);
    // Set seconds.
    cron.seconds("0");
    // Start the cronjob.
    CronJob::start_job_threaded(cron);

    let gitlab = Gitlab::from_env_vars();
    GITLAB.set(gitlab).unwrap();

    rocket::build().mount("/", routes![index])
}

// Our cronjob handler.
fn on_cron(name: &str) {
    println!("{}: It's time!", name);
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn gitlab_api_is_still_sane() {
        dotenv().ok();
        let gitlab = Gitlab::from_env_vars();

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
        let gitlab = Gitlab::from_env_vars();

        let result = gitlab
            .get_events(
                time::macros::date!(2024 - 05 - 03),
                time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
            )
            .await;
        assert_eq!(result.len(), 4);
    }
}
