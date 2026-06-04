use std::{
    env, io::{self, Read, Write}, net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs}, str::from_utf8, thread
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
        Self { id, address, nickname, created_at}
    }
}

fn start_server() {
    let address = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:6969".to_string());

    let listener = TcpListener::bind(&address).expect("Failed to bind server address");
    println!("Server listening on {address}");

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                thread::spawn(move || {
                    handle_client(&stream);
                });
            }
            Err(e) => {
                eprintln!("Failed to accept connection: {}", e);
            }
        }
    }
}

fn handle_client(mut stream: &TcpStream) {
    let peer_address = stream
        .peer_addr()
        .map_or_else(|_| SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0000), |address| address.to_socket_addrs().expect("Client send wrong IP address").next().unwrap());
    let peer_nickname = "".to_string();

    let user_profile = UserProfile::new(Uuid::new_v4(), peer_address, peer_nickname, Utc::now());

    let address = user_profile.address;
    let mut nickname = user_profile.nickname;
    let mut buffer = [0; 1024];
    
    println!("Handling connection from: '{address}'");

    if nickname == "" {
        nickname = ask_user_nickname(stream, buffer, &address);
    }

    println!("Address '{address}' successfuly set nickname to: {nickname}");

    loop {
        match stream.read(&mut buffer) {
            Ok(n) => {
                if n == 0 {
                    println!("Client {address} ({nickname}) closed connection");
                    break;
                }

                let user_message = from_utf8(&buffer[0..n]).unwrap_or("Invalid UTF-8").trim_end();
                println!("Message from {address} ({nickname}): {user_message}");
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                match e.kind() {
                    io::ErrorKind::ConnectionReset => {
                        println!("Client {address} ({nickname}) reset connection");
                    }
                    _ => {
                        eprintln!("Failed to read from client {address} ({nickname}): {e}");
                    }
                }
                break;
            }
        }
    }
    println!("Connection finished for: {address} ({nickname})");
}

fn ask_user_nickname(
    mut stream: &TcpStream,
    mut buffer: [u8; 1024],
    address: &SocketAddr
) -> String {
    let ask_nickname_message = "Hello new user! Please enter your nickname: ";
    let mut result = String::new();

    loop {
        match stream.write_all(ask_nickname_message.as_bytes()) {
            Ok(()) => {
                match stream.read(&mut buffer) {
                    Ok(n) => {
                        if n == 0 {
                            println!("Client {address} closed connection");
                            return result;
                        }

                        let entered_nickname =
                            from_utf8(&buffer[0..n]).unwrap_or("Invalid UTF-8 for nickname");
                        result = entered_nickname.trim_end().to_string();
                        return result;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        match e.kind() {
                            io::ErrorKind::ConnectionReset => {
                                println!("Client {address} reset connection");
                            }
                            _ => {
                                eprintln!("Failed to read from client {address}: {e}");
                            }
                        }
                        return result;
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                match e.kind() {
                    io::ErrorKind::ConnectionReset => {
                        println!("Client {address} reset connection");
                    }
                    _ => {
                        eprintln!("Failed to read from client {address}: {e}");
                    }
                }
                return result;
            }
        }
    }
}
