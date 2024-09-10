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

use core::str;
use std::borrow::Borrow;
use std::iter;

use dotenv::dotenv;
use once_cell::sync::OnceCell;
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

    pub async fn get_events(&self, after: Date, before: Date) -> String {
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

        let res = client.get(url).bearer_auth(token).send().await;

        //String::from(res.unwrap().status().as_str())
        let payload = match res {
            Ok(res) => match res.text().await {
                Ok(text) => text,
                Err(err) => panic!("Unable to decode response from Gitlab: {}", err),
            },
            Err(err) => panic!("Unable to send request! {}", err),
        };
        println!("{:?}", payload); // GET HEADERS FOR PAGINATION!!

        let data: Vec<GitlabEvent> = match serde_json::from_str(&payload) {
            Ok(data) => data,
            Err(err) => panic!(
                "Unable to decode json response from Gitlab: {}\nThis is what we received:\n{}",
                err, payload
            ),
        };

        for i in data {
            println!("{:?}", i);
        }
        //data.first().unwrap().project_id.to_string()
        "hello".to_string()
    }
}

static GITLAB: OnceCell<Gitlab> = OnceCell::new();

#[get("/")]
async fn index() -> (Status, (ContentType, String)) {
    //unsafe { format!("Hello, world! {}, {}", MY_COUNTER.unwrap_or(0)) }
    (
        Status::ImATeapot,
        (
            ContentType::JSON,
            Gitlab::global()
                .get_events(
                    (OffsetDateTime::now_utc() + Duration::days(-2)).date(),
                    (OffsetDateTime::now_utc() + Duration::days(0)).date(),
                )
                .await,
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
                time::macros::date!(2024 - 06 - 01), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
            )
            .await;
        assert_eq!(result, OffsetDateTime::now_utc().date().to_string())
    }
}
