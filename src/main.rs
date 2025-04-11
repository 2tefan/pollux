#[macro_use]
extern crate rocket;


mod gitlab;
mod github;
mod database;
mod git_platform;

use cronjob::CronJob;
use dotenv::dotenv;
use git_platform::GitPlatform;
use github::Github;
use gitlab::Gitlab;
use rocket::http::{ContentType, Status};
use time::{Duration, OffsetDateTime};
use tokio::join;
use serde::Serialize;
use rocket::serde::json::Json;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn fetch_data_from_git_providers() {
    let mut github = Github::init_from_env_vars();
    let gitlab = Gitlab::init_from_env_vars();


    let (_github_result, _gitlab_result) = join!(
        async {
            let events = github.get_events().await;
            github.insert_github_events_into_db(events).await;
        },
        async {
            let events = gitlab
                .get_events(
                    (OffsetDateTime::now_utc() + Duration::days(-90)).date(), // TODO: Remove magic number
                    (OffsetDateTime::now_utc()).date(),
                )
                .await;
            gitlab.insert_gitlab_events_into_db(events).await;
        }
    );
}

#[get("/health")]
fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[get("/force-sync")]
async fn force_sync() -> (Status, (ContentType, String)) {
    let dev_mode = std::env::var("POLLUX_ENABLE_DEV_MODE");
    if dev_mode.is_ok() && dev_mode.unwrap().to_ascii_lowercase() == "true" {
        fetch_data_from_git_providers().await;
        (
            Status::Ok,
            (
                ContentType::Text,
                "fetching done".to_string(),
            ),
        )
    } else { 
        (
            Status::Forbidden,
            (
                ContentType::Text,
                "Not allowed in prod!".to_string(),
            ),
        )
    }
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

    Gitlab::get_or_init();
    Github::get_or_init();

    rocket::build().mount("/", routes![force_sync, health])
}

// Our cronjob handler.
fn on_cron(name: &str) {
    println!("{}: It's time!", name);
}
