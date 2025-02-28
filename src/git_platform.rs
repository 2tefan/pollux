use chrono::{DateTime, Utc};
use log::trace;
use rocket::futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Row, Transaction};
use time::{format_description, OffsetDateTime};

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitProject {
    pub id: u64,
    pub platform_project_id: u64,
    pub name: String,
    pub url: String,
}

pub trait GitEventAPI {}

pub trait GitPlatform {
    const GIT_PLATFORM_ID: &'static str;
    type GitEventAPI: GitEventAPI;

    fn init_from_env_vars() -> Self;

    // pub fn get_or_init() {
    //     GITHUB.get_or_init(|| Self::init_from_env_vars());
    // }

    async fn get_events(&mut self) -> Vec<Self::GitEventAPI>;

    async fn set_platform(tx: &mut Transaction<'static, MySql>) {
        let rows = sqlx::query("SELECT name FROM GitPlatforms WHERE name = ?")
            .bind(Self::GIT_PLATFORM_ID)
            .fetch_all(&mut **tx) // Use fetch_all to collect all rows immediately
            .await
            .unwrap();

        if rows.len() > 1 {
            panic!(
                "There are more than 1x platforms with the same name! (name={}) - This can't be!",
                Self::GIT_PLATFORM_ID
            );
        }

        // Add platform, if it not yet exists
        if rows.is_empty() {
            let format =
                format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]").unwrap();
            sqlx::query("INSERT INTO GitPlatforms (name, firstSync) VALUES ( ?, ? )")
                .bind(Self::GIT_PLATFORM_ID)
                .bind(OffsetDateTime::now_utc().format(&format).unwrap())
                .execute(&mut **tx)
                .await
                .unwrap();
        }
    }

    async fn get_git_action_by_name(
        tx: &mut Transaction<'static, MySql>,
        action_name: &String,
    ) -> Option<u64> {
        let mut rows = sqlx::query("SELECT id FROM GitActions WHERE name = ?")
            .bind(action_name)
            .fetch(&mut **tx);

        let mut number_of_actions = 0;
        let mut git_action_id = Option::None;
        while let Some(row) = rows.try_next().await.unwrap() {
            if number_of_actions > 0 {
                error!(
                    "There are more than 1x Git Actions with the same name! (name={}) - skipping this event!",
                    action_name
                );
                return Option::None;
            }

            number_of_actions += 1;
            git_action_id = Some(row.try_get("id").unwrap());
        }

        git_action_id
    }

    async fn count_all_matching_events(
        tx: &mut Transaction<'static, MySql>,
        datetime: &DateTime<Utc>,
        action_id: &u64,
        project_id: &u64,
    ) -> i64 {
        // i64 needed by sqlx return type
        let result = sqlx::query(
                "SELECT COUNT(1) AS CNT FROM GitEvents AS ge, Events AS e \
                WHERE ge.id = e.id \
                AND e.timestamp = ? \
                AND ge.project_fk = ? \
                AND ge.action_fk = ?",
            )
            .bind(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
            .bind(project_id)
            .bind(action_id)
            .fetch_one(&mut **tx);

        let query_option = Some(result.await.unwrap().try_get("CNT").unwrap());

        if query_option.is_none() {
            return 0;
        }

        let number_of_rows = query_option.unwrap();

        if number_of_rows > 1 {
            error!(
                "There are {}x events with the same action (id={}) on the same project (id={}) at the same time ({}). \
                This means there are already duplicate events in your DB!",
                number_of_rows, action_id, project_id, datetime
            );
        }

        number_of_rows
    }

    async fn fetch_single_git_project_from_db(
        tx: &mut Transaction<'static, MySql>,
        platform_project_id: u64,
    ) -> Option<GitProject> {
        let mut rows =
            sqlx::query("SELECT id, platform_project_id, name, url FROM GitProjects WHERE platform_project_id = ? AND platform = ?")
                .bind(platform_project_id)
                .bind(Self::GIT_PLATFORM_ID)
                .fetch(&mut **tx);

        let mut number_of_projects = 0;
        let mut github_project = Option::None;
        while let Some(row) = rows.try_next().await.unwrap() {
            if number_of_projects > 0 {
                error!(
                    "There are more than 1x Git projects in DB (id={}, platform={}) - skipping this event!",
                    platform_project_id, Self::GIT_PLATFORM_ID
                );
                return Option::None;
            }

            number_of_projects += 1;
            let id: u64 = row.try_get("id").unwrap();
            let platform_project_id: u64 = row.try_get("platform_project_id").unwrap();
            let name: &str = row.try_get("name").unwrap();
            let url: &str = row.try_get("url").unwrap();
            github_project = Some(GitProject {
                id,
                platform_project_id,
                name: name.to_string(),
                url: url.to_string(),
            });
        }

        github_project
    }

    async fn write_project_to_db(
        &self,
        tx: &mut Transaction<'static, MySql>,
        project: &GitProject,
    ) -> u64 {
        Self::set_platform(tx).await; // TODO: Only do this at initial setup

        let project_id =
            sqlx::query("INSERT INTO GitProjects (platform, platform_project_id, name, url) VALUES ( ?, ?, ?, ? )")
                .bind(Self::GIT_PLATFORM_ID)
                .bind(project.id.clone())
                .bind(project.name.clone())
                .bind(project.url.clone())
                .execute(&mut **tx)
                .await
                .unwrap()
                .last_insert_id();
        trace!(
            "Inserted GitProject ({}) id: {}",
            Self::GIT_PLATFORM_ID,
            project_id
        );
        project_id
    }

    async fn insert_git_action(tx: &mut Transaction<'static, MySql>, action_name: &String) -> u64 {
        let action_id = sqlx::query("INSERT INTO GitActions (name) VALUES ( ? )")
            .bind(action_name)
            .execute(&mut **tx)
            .await
            .unwrap()
            .last_insert_id();
        trace!(
            "Inserted Git action ({}) - id: {} ({})",
            Self::GIT_PLATFORM_ID,
            action_id,
            action_name
        );
        return action_id;
    }

    async fn insert_event(tx: &mut Transaction<'static, MySql>, datetime: DateTime<Utc>) -> u64 {
        let event_id = sqlx::query("INSERT INTO Events (timestamp) VALUES ( ? )")
            .bind(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
            .execute(&mut **tx)
            .await
            .unwrap()
            .last_insert_id();
        trace!(
            "Inserted Git event ({}) - id: {} @ {}",
            Self::GIT_PLATFORM_ID,
            event_id,
            datetime
        );
        return event_id;
    }

    async fn insert_git_event(
        tx: &mut Transaction<'static, MySql>,
        event_id: u64,
        action_id: u64,
        project_id: u64,
    ) -> u64 {
        sqlx::query("INSERT INTO GitEvents (id, action_fk, project_fk) VALUES ( ?, ?, ? )")
            .bind(event_id)
            .bind(action_id)
            .bind(project_id)
            .execute(&mut **tx)
            .await
            .unwrap()
            .last_insert_id()
    }

    // // // TODO
    // pub async fn insert_github_events_into_db(&self, events: Vec<GithubEvent>) {
    //     let db = database::Database::get_or_init().await;
    //     let pool = db.get_pool().await;

    //     for event in events.iter() {
    //         // Starting transaction 💪
    //         let mut tx = pool.begin().await.expect("Couldn't start transaction!");
    //         let tx_ref = tx.borrow_mut();

    //         // TODO: Maybe check if name is still up-to-date etc.
    //         let github_project_option_future =
    //             Github::fetch_single_github_project_from_db(tx_ref, event.repo.id);

    //         let datetime: DateTime<Utc> = match event.created_at.parse() {
    //             Ok(datetime) => datetime,
    //             Err(err) => {
    //                 // Parsing failed - https://docs.rs/chrono/latest/chrono/struct.DateTime.html#impl-FromStr-for-DateTime%3CUtc%3E
    //                 error!("Couldn't parse date from Github using a relaxed form of RFC3339. Event will be skipped! Received 'created_at' value: {} - error msg: {}", event.created_at, err);
    //                 continue;
    //             }
    //         };

    //         // Inserting GithubProject
    //         // TODO fetching name + url from github and insert it, if missing
    //         let project_id = if let Some(project) = github_project_option_future.await {
    //             project.id
    //         } else {
    //             self.write_project_to_db(tx_ref, &event.repo)
    //                 .await
    //         };

    //         // TODO: Handle push_data (multiple commits!)
    //         let action_id =
    //             match Github::get_github_action_by_name(tx_ref, &event.type_of_action).await {
    //                 Some(value) => value,
    //                 None => Github::insert_github_action(tx_ref, &event.type_of_action).await,
    //             };

    //         // Add event itself
    //         let event_id = Github::insert_event(tx_ref, datetime).await;

    //         let github_event_id =
    //             Github::insert_github_event(&mut tx, event_id, action_id, project_id).await;

    //         tx.commit().await.expect("Couldn't apply transaction ._.");
    //     }
    // }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use dotenv::dotenv;

//     #[tokio::test]
//     async fn github_api_is_still_sane() {
//         dotenv().ok();
//         let mut github = Github::init_from_env_vars();

//         let result = github.get_events().await;
//         //assert_eq!(result, OffsetDateTime::now_utc().date().to_string())
//         assert_eq!(result.len(), 12);
//     }

//     #[tokio::test]
//     async fn github_api_is_still_sane_using_etag() {
//         dotenv().ok();
//         let mut github = Github::init_from_env_vars();

//         let result = github.get_events().await;
//         let result_not_modified = github.get_events().await;
//         //assert_eq!(result, OffsetDateTime::now_utc().date().to_string())
//         assert_eq!(result.len(), 12);
//         assert_eq!(result_not_modified.len(), 0);
//     }

//     #[tokio::test]
//     async fn import_data_from_github_into_database() {
//         dotenv().ok();
//         Github::get_or_init();
//         let mut github = Github::init_from_env_vars();

//         let events = github.get_events().await;
//         github.insert_github_events_into_db(events).await;
//     }
// }
