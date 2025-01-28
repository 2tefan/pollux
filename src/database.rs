
use sqlx::{MySql, Pool};
use tokio::sync::OnceCell;

pub static DATABASE: OnceCell<Database> = OnceCell::const_new();
pub(crate) struct Database {
    pool: sqlx::MySqlPool,
}

impl Database {
    pub async fn init_from_env_vars() -> Database {
        let db_user = std::env::var("MYSQL_USER").expect("Please specify MYSQL_USER as env var!");
        let db_password =
            std::env::var("MYSQL_PASSWORD").expect("Please specify MYSQL_PASSWORD as env var!");
        let db_host = std::env::var("MYSQL_HOST").expect("Please specify MYSQL_HOST as env var!");
        let db_port = std::env::var("MYSQL_PORT").expect("Please specify MYSQL_PORT as env var!");
        let db_target_database =
            std::env::var("MYSQL_DATABASE").expect("Please specify MYSQL_DATABASE as env var!");

        let pool = sqlx::MySqlPool::connect(
            format!(
                "mysql://{}:{}@{}:{}/{}",
                db_user, db_password, db_host, db_port, db_target_database
            )
            .as_str(),
        )
        .await
        .unwrap();

        match sqlx::migrate!().run(&pool).await {
            Ok(result) => result,
            Err(err) => panic!("Couldn't run db migrations: {}", err),
        }

        Database { pool }
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
