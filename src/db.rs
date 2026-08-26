use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use rusqlite::{Connection, OptionalExtension, Row, params};
use zxcvbn::Score;
use std::{fs, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};
use uuid::{Uuid};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

#[derive(Debug)]
pub struct User {
    pub id: Uuid,
    pub login: String,
    pub password: String,
    pub created_at: DateTime<Local>,
}

pub fn init_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
    Ok(conn)
}

pub fn get_db_path() -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("APPDATA not set")?;
        PathBuf::from(appdata.replace("Roaming", "LocalLow"))
    } else {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(xdg)
        } else {
            let home = std::env::var("HOME").context("HOME not set")?;
            PathBuf::from(home).join(".local/share")
        }
    };
    let db_dir = base_dir.join("Zeerck Inc").join("MUC-server").join("db");
    Ok(db_dir.join("database.sqlite"))
}

pub fn migrate(connection: &Connection) -> Result<()> {
    connection.execute(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id BLOB PRIMARY KEY NOT NULL,
            login TEXT NOT NULL UNIQUE,
            password TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )
        "#,
        params![],
    )?;

    connection.execute(
		r#"
		CREATE TABLE IF NOT EXISTS messages (
			id BLOB PRIMARY KEY NOT NULL,
			sender_id BLOB NOT NULL,
			receiver_id BLOB NOT NULL,
			content TEXT NOT NULL,
			timestamp INTEGER NOT NULL,
			is_read INTEGER DEFAULT 0,
			FOREIGN KEY(sender_id) REFERENCES users(id),
			FOREIGN KEY(receiver_id) REFERENCES messages(id)
      )
      "#,
      params![],
    )?;
    Ok(())
}

pub fn add_user(connection: &Connection, login: &str, raw_password: &str) -> Result<User> {
    if !validate_login(login) {
        anyhow::bail!("invalid login: must be 3-32 letters, numbers or underscores");
    }

    let entropy = zxcvbn::zxcvbn(raw_password, &[]);
    if entropy.score() < Score::Two {
        anyhow::bail!("password is too weak");
    }

    let id = Uuid::new_v4();
    let hashed_password = hash_password(raw_password).expect("Error while hashing password!");
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("System time before epoch").as_secs() as i64;

    connection.execute(
        "INSERT INTO users (id, login, password, created_at) VALUES (?, ?, ?, ?)",
        params![&id, &login, &hashed_password, &now],
    )?;

    let user = connection.query_row(
        "SELECT id, login, password, created_at FROM users WHERE id = ?",
        params![&id],
        map_row_to_user,
    )?;

    Ok(user)
}

pub fn get_user_by_login(connection: &Connection, login: &str) -> Result<Option<User>> {
    let mut stmt = connection.prepare("SELECT id, login, password, created_at FROM users WHERE login = ?")?;
    let user = stmt.query_row(params![login], map_row_to_user).optional()?;
    Ok(user)
}

pub fn get_user_by_id(connection: &Connection, id: Uuid) -> Result<Option<User>> {
    let mut stmt = connection.prepare("SELECT id, login, password, created_at FROM users WHERE id = ?")?;
    let user = stmt.query_row(params![&id], map_row_to_user).optional()?;
    Ok(user)
}

pub fn get_all_users(connection: &Connection) -> Result<Vec<User>> {
    let mut stmt = connection.prepare("SELECT id, login, password, created_at FROM users ORDER BY created_at")?;
    let users = stmt.query_map([], map_row_to_user)?.collect::<Result<Vec<_>, _>>()?;
    Ok(users)
}

pub fn delete_user(connection: &Connection, id: &Uuid) -> Result<bool> {
    let rows = connection.execute("DELETE FROM users where id = ?", params![id])?;
    Ok(rows > 0)
}

pub fn validate_login(nick: &str) -> bool {
    let len = nick.len();
    if len < 3 || len > 32 {
        return false;
    }
    nick.chars().all(|char| char.is_ascii_alphanumeric() || char == '_')
}

fn map_row_to_user(row: &Row) -> rusqlite::Result<User> {
    let id: Uuid = row.get(0)?;
    let login: String = row.get(1)?;
    let password: String = row.get(2)?;
    let timestamp: i64 = row.get(3)?;
    let dt_utc = match DateTime::from_timestamp_secs(timestamp) {
        Some(dt) => dt,
        None => return Err(rusqlite::Error::FromSqlConversionFailure(
           3, rusqlite::types::Type::Integer,
           Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid timestamp")), 
        )),
    };
    let created_at = dt_utc.with_timezone(&Local);

    Ok(User { id, login, password, created_at })
}

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(password_hash.to_string())
}

pub fn verify_password(password: &str, phc_hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(phc_hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default().verify_password(password.as_bytes(), &parsed_hash).is_ok()
}

pub fn save_message(
    connection: &Connection,
    sender_id: &Uuid,
    receiver_id: &Uuid,
    content: &str,
) -> Result<()> {
    let id = Uuid::new_v4();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before epoch")
        .as_secs() as i64;

    connection.execute(
        "INSERT INTO messages (id, sender_id, receiver_id, content, timestamp) VALUES (?, ?, ?, ?, ?)",
        params![&id, &sender_id, &receiver_id, &content, now],
    )?;
    Ok(())
}

pub fn get_undelivered_messages(
    connection: &Connection,
    user_id: &Uuid
) -> Result<Vec<(String, String)>> {
    let mut stmt = connection.prepare(
        "SELECT m.content, u.login
        FROM messages m
        JOIN users u ON m.sender_id = u.id
        WHERE m.receiver_id = ? AND m.is_read = 0
        ORDER BY m.timestamp ASC"
    )?;

    let messages = stmt.query_map(params![user_id], |row| {
        let content: String = row.get(0)?;
        let sender_login: String = row.get(1)?;
        Ok((sender_login, content))
    })?.collect::<Result<Vec<_>, _>>()?;

    connection.execute(
        "UPDATE messages SET is_read = 1 WHERE receiver_id = ?",
        params![user_id],
    )?;

    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_operations() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;

        let login = "Alice";
        let password = "best_password_123_A!";

        let user = add_user(&conn, login, &password)?;
        assert!(!user.id.is_nil());

        let fetched = get_user_by_login(&conn, login)?;
        assert!(fetched.is_some());
        let user = fetched.unwrap();
        assert_eq!(user.login, login);

        let all = get_all_users(&conn)?;
        assert_eq!(all.len(), 1);

        let deleted = delete_user(&conn, &user.id)?;
        assert!(deleted);

        let after_delete = get_user_by_login(&conn, login)?;
        assert!(after_delete.is_none());

        Ok(())
    }
}