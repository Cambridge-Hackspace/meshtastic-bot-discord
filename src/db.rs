use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};
use tracing::info;

pub async fn init_db(db_url: &str) -> SqlitePool {
    let db_opts = db_url
        .parse::<SqliteConnectOptions>()
        .unwrap()
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .connect_with(db_opts)
        .await
        .expect("Failed to connect to SQLite");

    // primary channels for each guild
    pool.execute(
        "CREATE TABLE IF NOT EXISTS guild_channels (
            guild_id TEXT PRIMARY KEY,
            channel_id TEXT NOT NULL,
            index_message_id TEXT
        );",
    )
    .await
    .expect("Failed to create guild_channels table.");

    // threads mapped to specific node IDs or secondary channels
    pool.execute(
        "CREATE TABLE IF NOT EXISTS mesh_threads (
            thread_id TEXT PRIMARY KEY,
            guild_id TEXT NOT NULL,
            destination TEXT NOT NULL
        );",
    )
    .await
    .expect("Failed to create mesh_threads table.");

    info!("Database initialize successfully.");
    pool
}
