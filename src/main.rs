mod logger;
use logger::prelude::*;

use std::{
    env, io::{self, Read, Write}, net::{SocketAddr, TcpListener, TcpStream}, str::from_utf8, thread
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

fn main() {
    start_server();
}

struct UserProfile {
    id: Uuid,
    address: SocketAddr,
    nickname: String,
    created_at: DateTime<Utc>,
}

impl UserProfile {
    fn new(id: Uuid, address: SocketAddr, nickname: String, created_at: DateTime<Utc>) -> Self {
        Self { id, address, nickname, created_at }
    }
}

fn start_server() {
    let address = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:6969".to_string());

    let listener = TcpListener::bind(&address).expect("Failed to bind server address");
    log(format!("Server listening on {address}"), Info);

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                // Передаём владение stream в поток
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
    let peer_addr = stream.peer_addr().unwrap();
    let nickname = peer_addr.port().to_string(); // временно используем порт как ник

    let user_profile = UserProfile::new(Uuid::new_v4(), peer_addr, nickname.clone(), Utc::now());

    log(format!("Handling connection from: {peer_addr}"), Trace);
    log(format!("Nickname set to: {nickname}"), Trace);

    let mut buffer = [0; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                log(format!("Client {} closed connection", peer_addr), Trace);
                break;
            }
            Ok(n) => {
                let msg = from_utf8(&buffer[0..n]).unwrap_or("Invalid UTF-8").trim_end();
                log(format!("Message from {nickname}: {msg}"), Info);
                // Опционально: можно отправить ответ клиенту
                let response = format!("Server received: {msg}\n");
                let _ = stream.write_all(response.as_bytes());
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                log(format!("Error reading from {peer_addr}: {e}"), Warning);
                break;
            }
        }
    }
    log(format!("Connection finished for: {peer_addr}"), Trace);
}