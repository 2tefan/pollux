
use std::time::Duration;

use log::{error, warn};
use sqlx::{mysql::{MySqlConnectOptions, MySqlPoolOptions}, MySql, MySqlPool, Pool};
use tokio::sync::OnceCell;

static FALLBACK_DB_RETRIES: i32 = 16;
static FALLBACK_MYSQL_PORT: u16 = 3306;

pub static DATABASE: OnceCell<Database> = OnceCell::const_new();
pub(crate) struct Database {
    pool: sqlx::MySqlPool,
}

impl Database {
    pub async fn init_from_env_vars() -> Database {
        let pool = Database::connect_with_retries().await;

        debug!("Running DB migrations!");
        match sqlx::migrate!().run(&pool).await {
            Ok(result) => result,
            Err(err) => panic!("Couldn't run db migrations: {}", err),
        }

        Database { pool }
    }


    async fn connect_with_retries() -> MySqlPool {
        let db_user = std::env::var("MYSQL_USER").expect("Please specify MYSQL_USER as env var!");
        let db_password =
            std::env::var("MYSQL_PASSWORD").expect("Please specify MYSQL_PASSWORD as env var!");
        let db_host = std::env::var("MYSQL_HOST").expect("Please specify MYSQL_HOST as env var!");
        let db_port = match std::env::var("MYSQL_PORT").expect("Please specify MYSQL_PORT as env var!").parse::<u16>() {
            Ok(result) => result,
            Err(err) => {
                error!("MYSQL_PORT is not a valid u16, falling back to {}: {}", FALLBACK_MYSQL_PORT, err);
                FALLBACK_MYSQL_PORT
            }
        };
        let db_target_database =
            std::env::var("MYSQL_DATABASE").expect("Please specify MYSQL_DATABASE as env var!");


        let max_retries =
            match std::env::var("POLLUX_DB_RETRIES").unwrap_or(FALLBACK_DB_RETRIES.to_string()).parse::<i32>() {
                Ok(result) => result,
                Err(err) => {
                    warn!("Unable to parse POLLUX_DB_RETRIES, using »{}« as a fallback: {}", FALLBACK_DB_RETRIES, err);
                    FALLBACK_DB_RETRIES
                }};
        let mut delay = 125;

        info!("Connecting to {}@{}:{}/{}", db_user, db_host, db_port, db_target_database);

        for attempt in 1..=max_retries {
            let connect_options = MySqlConnectOptions::new().host(&db_host).port(db_port).username(&db_user).password(&db_password).database(&db_target_database);
            match MySqlPoolOptions::new().acquire_timeout(Duration::from_millis(delay)).connect_with(connect_options).await {
                Ok(pool) => return pool,
                Err(err) if attempt < max_retries => {
                    error!("Attempt {}/{}: Failed to connect to DB: {}", attempt, max_retries, err);
                    //sleep(Duration::from_millis(delay)).await;
                    delay *= 2;
                }
                Err(err) => {
                    panic!("Failed to connect to DB after {} attempts: {}", max_retries, err);
                }
            }
        }

        unreachable!("Retry logic should have either returned or panicked");
    }

    pub async fn get_or_init() -> &'static Database {
        DATABASE.get_or_init(|| Self::init_from_env_vars()).await
    }

    pub async fn get_pool(&self) -> Pool<MySql> {
        self.pool.clone()
    }
}

#[cfg(test)]
mod tests {
    

    
    use dotenv::dotenv;
    
    use sqlx::MySql;
    use testcontainers::{
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
        GenericImage, ImageExt,
    };

    use std::sync::Once;
    static INIT: Once = Once::new();

    async fn initialize() -> (
        testcontainers::ContainerAsync<GenericImage>,
        sqlx::Pool<MySql>,
    ) {
        dotenv().ok();
        let db_user = std::env::var("MYSQL_USER").expect("Please specify MYSQL_USER as env var!");
        let db_password =
            std::env::var("MYSQL_PASSWORD").expect("Please specify MYSQL_PASSWORD as env var!");
        //let db_host = std::env::var("MYSQL_HOST").expect("Please specify MYSQL_HOST as env var!");
        let db_target_database =
            std::env::var("MYSQL_DATABASE").expect("Please specify MYSQL_DATABASE as env var!");

        let future_container = GenericImage::new(
            "mariadb",
            "latest@sha256:4a1de8fa2a929944373d7421105500ff6f889ce90dcb883fbb2fdb070e4d427e",
        )
        .with_exposed_port(3306.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server socket created on IP"))
        .with_env_var("MYSQL_USER", db_user.clone())
        .with_env_var("MYSQL_PASSWORD", db_password.clone())
        .with_env_var("MYSQL_DATABASE", db_target_database.clone())
        .with_env_var("MYSQL_RANDOM_ROOT_PASSWORD", "TRUE") // not needed here
        .start();

        //println!("Starting container...");

        let container = future_container.await.expect(
            "Couldn't start testcontainer! Check documentation if everything is setup correctly!",
        );

        //println!("Container up and running!");
        let host = container
            .get_host()
            .await
            .expect("Couldn't get host for testcontainer(?)");
        let port = container
            .get_host_port_ipv4(3306.tcp())
            .await
            .expect("Port 3306 not found on mariadb container! Check image");

        //println!(
        //    "mysql://{}:{}@{}:{}/{}",
        //    db_user, db_password, host, port, db_target_database
        //);
        let pool = sqlx::MySqlPool::connect(
            format!(
                "mysql://{}:{}@{}:{}/{}",
                db_user, db_password, host, port, db_target_database
            )
            .as_str(),
        )
        .await
        .unwrap();

        match sqlx::migrate!().run(&pool).await {
            Ok(result) => result,
            Err(err) => panic!("Couldn't run db migrations: {}", err),
        }

        // We have to return both pool and container
        // Otherwise container will be stopped, if it goes out-of-scope
        return (container, pool);
    }

    #[tokio::test]
    async fn check_if_db_is_alive() {
        initialize().await;
    }

    #[tokio::test]
    async fn do_tables_exists() {
        let (_container, pool) = initialize().await;
        let tables: Vec<(String,)> = sqlx::query_as("SHOW TABLES")
            .fetch_all(&pool)
            .await
            .unwrap();

        // for table in tables.iter() {
        //     println!("{}", table.0);
        //     println!("{:?}", table);
        // }

        assert!(tables.len() > 0);
    }

    #[tokio::test]
    async fn run_migrations_twice() {
        let (_container, pool) = initialize().await;

        assert!(sqlx::migrate!().run(&pool).await.is_ok());
    }
}
