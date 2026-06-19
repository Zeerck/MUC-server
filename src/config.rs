use crate::{error, db};
use std::{env, path::PathBuf, time::Duration};

#[derive(Debug)]
pub struct Config {
    pub server_address: String,
    pub app_name: String,
    pub db_path: PathBuf,
    pub nickname_timeout: u64,
    pub read_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Self {
        let server_address = env::var("SERVER_ADDRESS").unwrap_or_else(|error| {
            error!("Failed to get 'SERVER_ADDRESS' from .env: {error}");
            "127.0.0.1:6969".to_string()
        });

        let app_name = env::var("APP_NAME").unwrap_or_else(|error| {
            error!("Failed to get 'APP_NAME' from .env: {error}");
            "MUC-server".to_string()
        });

        let db_path = env::var("DB_PATH")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                db::get_db_path(&app_name).expect("Failed to build default DB path")
            });

        let nickname_timeout = env::var("NICKNAME_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        let read_timeout = env::var("READ_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&t| t > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(300));

        Self {
            server_address,
            app_name,
            db_path,
            nickname_timeout,
            read_timeout,
        }
    }
}
