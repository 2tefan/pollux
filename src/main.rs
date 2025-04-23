#[macro_use]
extern crate rocket;

mod database;
mod git_platform;
mod github;
mod gitlab;


use std::time::Duration;

use chrono::{NaiveDate, Utc};
use dotenv::dotenv;
use git_platform::{GitEvents, GitPlatform};
use github::Github;
use gitlab::Gitlab;
use log::info;
use rocket::http::{ContentType, Status};
use rocket::serde::json::Json;
use serde::Serialize;
use tokio::join;
use tokio::time::sleep;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn fetch_data_from_git_providers() {
    let github_arc = Github::get_or_init();
    let gitlab_arc = Gitlab::get_or_init();

    let (_github_result, _gitlab_result) = join!(
        async {
            let mut github = github_arc.lock().await;
            github.update_provider().await;
        },
        async {
            let mut gitlab = gitlab_arc.lock().await;
            gitlab.update_provider().await;
        }
    );
}

#[get("/health")]
fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[get("/git-events?<since..>")]
async fn get_git_events(since: Option<&str>) -> Json<Vec<GitEvents>> {
    let date = match since {
        Some(input) => {
            match NaiveDate::parse_from_str(input, "%Y-%m-%d") {
                Ok(result) => result,
                Err(err) => {
                    warn!("Couldn't parse {} as a date. Falling back to last 30 days: {}", input, err);
                    (Utc::now() - chrono::Duration::days(30)).date_naive()
                }
            }
        }
        None => 
        {
            debug!("Using default of 30 days...");
            (Utc::now() - chrono::Duration::days(30)).date_naive()
        }
    };

    info!("Getting events since {}", date);

    Json(Gitlab::get_all_git_events(date).await)
}

#[get("/force-sync")]
async fn force_sync() -> (Status, (ContentType, String)) {
    let dev_mode = std::env::var("POLLUX_ENABLE_DEV_MODE");
    if dev_mode.is_ok() && dev_mode.unwrap().to_ascii_lowercase() == "true" {
        fetch_data_from_git_providers().await;
        (Status::Ok, (ContentType::Text, "fetching done".to_string()))
    } else {
        (
            Status::Forbidden,
            (ContentType::Text, "Not allowed in prod!".to_string()),
        )
    }
}

async fn run_cron_job() {
    let resync_timeout_hours = match std::env::var("POLLUX_RESYNC_TIMEOUT_HOURS").expect("Please specify POLLUX_RESYNC_TIMEOUT_HOURS as env var!").parse::<u64>() {
        Ok(result) => result,
        Err(err) => {
            panic!("POLLUX_RESYNC_TIMEOUT_HOURS is not a valid u64! Please set it to a valid positive integer: {}", err);
        }
    };
    loop {
        info!("Crontime ✨");

        // Run the actual fetching
        fetch_data_from_git_providers().await;

        sleep(Duration::new(resync_timeout_hours * 3600, 0)).await;
    }
}

#[rocket::main]
async fn main() -> Result<(), rocket::Error> {
    dotenv().ok();
    env_logger::init();

    // Init git providers
    Gitlab::get_or_init();
    Github::get_or_init();

    // Prepare cronjob
    tokio::spawn(async {
        run_cron_job().await
    });

    rocket::build()
        .mount("/", routes![health])
        .mount("/api/v1", routes![force_sync, get_git_events])
        .launch()
        .await
        .unwrap();

    Ok(())
}

