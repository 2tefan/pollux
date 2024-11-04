#[cfg(test)]
mod tests {
    use super::*;
    use dotenv::dotenv;

    #[tokio::test]
    async fn check_if_db_is_alive() {
        dotenv().ok();
        let db_user = std::env::var("MYSQL_USER").expect("Please specify MYSQL_USER as env var!");
        let db_password =
            std::env::var("MYSQL_PASSWORD").expect("Please specify MYSQL_PASSWORD as env var!");
        let db_host = std::env::var("MYSQL_HOST").expect("Please specify MYSQL_HOST as env var!");
        let db_target_database =
            std::env::var("MYSQL_DATABASE").expect("Please specify MYSQL_DATABASE as env var!");

        let pool = sqlx::MySqlPool::connect(
            format!(
                "mysql://{}:{}@{}/{}",
                db_user, db_password, db_host, db_target_database
            )
            .as_str(),
        )
        .await.unwrap();


        let tables: Vec<(String,)> = sqlx::query_as("SHOW TABLES").fetch_all(&pool).await.unwrap();

        //for table in tables.iter() {
        //    println!("{}", table.0);
        //    println!("{:?}", table);
        //}

        assert!(tables.len() > 0);

        // TODO: Do this
    }
}
