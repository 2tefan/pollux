#[macro_use]
extern crate rocket;

mod database;
mod git_platform;
mod github;
mod gitlab;

use std::time::Duration;

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
    let mut github = Github::init_from_env_vars();
    let mut gitlab = Gitlab::init_from_env_vars();

    let (_github_result, _gitlab_result) = join!(
        async {
            github.update_provider().await;
        },
        async {
            gitlab.update_provider().await;
        }
    );
}

#[get("/health")]
fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[get("/git-events")]
async fn get_git_events() -> Json<Vec<GitEvents>> {
    Json(Gitlab::get_all_git_events().await)
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
    loop {
        sleep(Duration::new(1, 0)).await;
        info!("Crontime ✨");

        // Run the actual fetching
        //fetch_data_from_git_providers().await;
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

