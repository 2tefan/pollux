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

#[get("/")]
async fn index() -> (Status, (ContentType, String)) {
    dotenv().ok();
    let mut github = Github::init_from_env_vars();
    let gitlab = Gitlab::init_from_env_vars();

    let events = github.get_events().await;
    github.insert_github_events_into_db(events).await;


    let events = gitlab
        .get_events(
            time::macros::date!(2024 - 05 - 03),
            time::macros::date!(2024 - 05 - 05), // (OffsetDateTime::now_utc() + Duration::days(-85)).date(),
        )
        .await;
    gitlab.insert_gitlab_events_into_db(events).await;
    (
        Status::Ok,
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

    Gitlab::get_or_init();

    rocket::build().mount("/", routes![index])
}

// Our cronjob handler.
fn on_cron(name: &str) {
    println!("{}: It's time!", name);
}
