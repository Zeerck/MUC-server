mod commands;
mod config;
mod db;
mod logger;

use commands::Command;
use dotenvy::dotenv;

use std::{
    io::{self, BufRead, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    sync::{Arc, LazyLock, Mutex},
    thread, time::Duration,
};

use crate::{commands::handle_command, config::Config};

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    Config::from_env()
});

fn main() {
    dotenv().ok();

    let listener = TcpListener::bind(&CONFIG.server_address)
        .expect("Failed to bind listener");
    info!("Server listening on {}", &CONFIG.server_address);

    let connection = db::init_database(&CONFIG.db_path)
        .expect("Failed to open database");
    info!("Database connected");

    if let Err(e) = db::migrate(&connection) {
        fatal!("Migration failed: {e}");
        return;
    }

    let db_arc = Arc::new(Mutex::new(connection));

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                let db_clone = db_arc.clone();
                thread::spawn(move || {
                    handle_client(stream, db_clone);
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

fn handle_client(mut stream: TcpStream, db: Arc<Mutex<rusqlite::Connection>>) {
    let peer_address = match stream.peer_addr() {
        Ok(address) => address,
        Err(e) => {
            error!("Failed to get peer address: {e}");
            return;
        }
    };

    trace!("Handling connection from: {peer_address}");

    let _ = stream.set_read_timeout(Some(CONFIG.read_timeout));

    let user = {
        let connection = db.lock().unwrap();
        match get_or_create_user(&connection, &mut stream, peer_address) {
            Ok(u) => u,
            Err(e) => {
                error!("Failed to get/create user: {e}");
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
        }
    };

    let user_address = user.address.clone();
    let user_nickname = user.nickname.clone();

    let mut buffer = [0; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                trace!("Client {user_address} ({user_nickname}) closed connection");
                break;
            }
            Ok(n) => {
                let msg = from_utf8(&buffer[0..n])
                    .unwrap_or("Invalid UTF-8")
                    .trim_end();
                info!("Message from {user_nickname} ({user_address}): {msg}");

                if let Some(command) = Command::from_str(msg) {
                    let should_continue = match stream.try_clone() {
                        Ok(cloned) => handle_command(cloned, command),
                        Err(e) => {
                            error!("Failed to clone stream: {e}");
                            false
                        }
                    };

                    if !should_continue {
                        break;
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                warning!("Error reading from {user_address} ({user_nickname}): {e}");
                break;
            }
        }
    }
    trace!("Connection finished for: {user_address} ({user_nickname})");
}

fn request_nickname(mut stream: &TcpStream) -> Result<String, io::Error> {
    stream.set_read_timeout(Some(Duration::from_secs(CONFIG.nickname_timeout)))?;
    let mut reader = std::io::BufReader::new(stream.try_clone()?);
    let mut nickname = String::new();
    let request_nickname =
        format!("Please, enter your nickname (timeout within {} secs): ", CONFIG.nickname_timeout);
    stream.write_all(request_nickname.as_bytes())?;

    match reader.read_line(&mut nickname) {
        Ok(0) | Err(_) => {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Timeout or EOF",
            ));
        }
        Ok(_) => {
            let trimmed = nickname.trim();
            if db::validate_nickname(trimmed) {
                let _ = stream.set_read_timeout(Some(CONFIG.read_timeout));
                Ok(trimmed.to_string())
            } else {
                let _ = stream.write_all(b"Invalid nickname. Disconnected.\n");
                let _ = stream.shutdown(Shutdown::Both);
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid nickname",
                ))
            }
        }
    }
}

fn get_or_create_user(
    connection: &rusqlite::Connection,
    stream: &mut TcpStream,
    address: SocketAddr,
) -> Result<db::User, anyhow::Error> {
    trace!("Trying to find user with address '{address}' in Database...");
    if let Some(user) = db::get_user_by_address(connection, &address)? {
        info!(
            "Found user by address: {:?} ({})",
            user.nickname, user.address
        );
        return Ok(user);
    }
    trace!("User with address '{address}' not found in database");

    trace!("Requesting user '{address}' nickname...");
    let requested_nickname = match request_nickname(stream) {
        Ok(nick) => nick,
        Err(e) => {
            warning!("Nickname requset failed: {e}");
            anyhow::bail!("Nickname request failed: {e}");
        }
    };

    trace!("Successfully getted user nickname: '{requested_nickname}' ({address})");
    trace!("Trying to find user '{requested_nickname}' ({address}) in Database...");

    if let Some(existing) = db::get_user_by_nickname(connection, &requested_nickname)? {
        warning!(
            "Nickname '{}' already taken by {}",
            requested_nickname,
            existing.address
        );

        let _ = stream.write_all(b"Nickname already taken. Disconnecting.\n");
        let _ = stream.shutdown(Shutdown::Both);
        anyhow::bail!("Nickname already taken");
    }

    trace!("User '{requested_nickname}' ({address}) not found in Database");
    trace!(
        "Trying add user with nickname: '{requested_nickname}' and address: {address} to Database..."
    );

    let user = db::add_user(connection, address, &requested_nickname)?;
    trace!(
        "User inserted with ID: '{}' and nickname '{}'",
        user.id, user.nickname
    );
    Ok(user)
}
