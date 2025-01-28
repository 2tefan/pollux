#[macro_use]
extern crate rocket;


mod gitlab;
mod github;
mod database;
mod git_platform;

use cronjob::CronJob;
use dotenv::dotenv;
use gitlab::Gitlab;
use rocket::http::{ContentType, Status};

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

    Gitlab::get_or_init();

    rocket::build().mount("/", routes![index])
}

// Our cronjob handler.
fn on_cron(name: &str) {
    println!("{}: It's time!", name);
}
