use crate::{db, trace};
use std::{env, path::PathBuf, time::Duration};

#[derive(Debug)]
pub struct Config {
    pub server_address: String,
    pub db_path: PathBuf,
    pub read_timeout: Duration,
    pub handshake_timeout: Duration,
    pub tls_cert_path: PathBuf,
    pub tls_key_path: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        let server_address = env::var("SERVER_ADDRESS")
            .unwrap_or_else(|_| {
                trace!("Environment parameter 'SERVER_ADDRESS' not found. Default address and port are being used: 0.0.0.0:1990");
                "0.0.0.0:1990".to_string()
            });

        let db_path = match env::var("DB_PATH") {
            Ok(val) if !val.trim().is_empty() => PathBuf::from(val),
            _ => db::get_db_path().expect("Failed to build default DB path"),
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
        
        let tls_cert_path = env::var("TLS_CERT_PATH")
            .map(PathBuf::from)
            .expect("TLS_CERT_PATH variable is required");

        let tls_key_path = env::var("TLS_KEY_PATH")
            .map(PathBuf::from)
            .expect("TLS_KEY_PATH variable is required");

        Self {
            server_address,
            db_path,
            read_timeout,
            handshake_timeout,
            tls_cert_path,
            tls_key_path,
        }
    }
}