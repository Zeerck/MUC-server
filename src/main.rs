mod db;
mod logger;
mod commands;
use logger::prelude::*;
use commands::Command;

use std::{
    env,
    io::{self, BufRead, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    thread,
    time::Duration,
    io::Error as IOError
};

use crate::commands::handle_command;

fn main() {
    start_server();
}

fn start_server() {
    let address = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:6969".to_string());

    let listener = TcpListener::bind(&address).expect("Failed to bind server address");
    log(format!("Server listening on {address}"), Info);

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                thread::spawn(move || {
                    handle_client(stream);
                });
            }
            Err(e) => {
                log(format!("Failed to accept connection: {e}"), Error);
            }
        }
    }
}

fn handle_client(mut stream: TcpStream) {
    let peer_address = stream.peer_addr().unwrap();
    log(format!("Handling connection from: {peer_address}"), Trace);

    let _ = stream.set_read_timeout(None);
    let user = get_or_create_user(stream.try_clone().expect("Stream has blocked in handle_client"), peer_address, None);
    let user_address = user.address;
    let user_nickname = user.nickname;

    let mut buffer = [0; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                log(
                    format!("Client {user_address} ({user_nickname}) closed connection"),
                    Trace,
                );
                break;
            }
            Ok(n) => {
                let msg = from_utf8(&buffer[0..n])
                    .unwrap_or("Invalid UTF-8")
                    .trim_end();
                log(
                    format!("Message from {user_nickname} ({user_address}): {msg}"),
                    Info,
                );

                if let Some(command) = Command::from_str(msg) {
                    handle_command(stream.try_clone().expect("Stream has blocked in handle_client"), command);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                log(
                    format!("Error reading from {user_address} ({user_nickname}): {e}"),
                    Warning,
                );
                break;
            }
        }
    }
    log(
        format!("Connection finished for: {user_address} ({user_nickname})"),
        Trace,
    );
}

fn request_nickname(mut stream: TcpStream) -> Result<String, IOError> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
    let mut nickname = String::new();
    let request_nickname = "Please, enter your nickname (timeout within 15 secs): ";
    let _ = stream.write_all(request_nickname.as_bytes());

    match reader.read_line(&mut nickname) {
        Ok(0) | Err(_) => {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Disconnected: timeout while handshake with client",
            ))
            .into();
        }
        Ok(_) => Ok(nickname.trim().to_string()),
    }
}

#[tokio::main]
pub async fn get_or_create_user(stream: TcpStream, address: SocketAddr, nickname: Option<&str>) -> db::User {
    let pool = db::init_database(env!("CARGO_PKG_NAME")).await.unwrap();
    let _ = db::migrate(&pool).await;

    if let Some(user) = db::get_user_by_address(&pool, &address).await.unwrap() {
        log(format!("Found user by address: {:?} ({})", user.nickname, user.address), Info);
        return user;
    } else if nickname != None && let Some(user) = db::get_user_by_nickname(&pool, nickname.unwrap()).await.unwrap() {
        log(format!("Found user by nickname: {:?} ({})", user.nickname, user.address), Info);
        return user;
    } else {
        let requested_nickname = &match request_nickname(stream) {
            Ok(requested_nickname) => requested_nickname.to_string(),
            Err(e) => {
                log(format!("Client '{address}' connection timeout!"), Warning);
                Err(e).expect(&format!("Client '{address}' connection timeout!").to_string())
            },
        }.to_string();

        let user = db::add_user(&pool, address, requested_nickname).await.unwrap();
        log(format!("User inserted with ID: '{}' and nickname '{}'", user.id, user.nickname), Info);
        return user;
    }
}