use crate::{db, trace};
use std::{env, path::PathBuf, time::Duration};

#[derive(Debug)]
pub struct Config {
    pub server_address: String,
    pub app_name: String,
    pub db_path: PathBuf,
    pub read_timeout: Duration,
    pub handshake_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Self {
        // Используем ok() чтобы игнорировать отсутствие переменной без паники и ошибок
        let server_address = env::var("SERVER_ADDRESS")
            .unwrap_or_else(|_| {
                trace!("Environment parameter 'SERVER_ADDRESS' not found. Default address and port are being used: 0.0.0.0:1990.");
                "0.0.0.0:1990".to_string()
            });

        let app_name = env::var("APP_NAME")
            .unwrap_or_else(|_| "MUC-server".to_string());

        let db_path = match env::var("DB_PATH") {
            Ok(val) if !val.trim().is_empty() => PathBuf::from(val),
            _ => db::get_db_path(&app_name).expect("Failed to build default DB path"),
        };

        let read_timeout = env::var("READ_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&t| t > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(300));

        let handshake_timeout = env::var("HANDSHAKE_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&t| t > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(10));

        Self {
            server_address,
            app_name,
            db_path,
            read_timeout,
            handshake_timeout,
        }
    }
}