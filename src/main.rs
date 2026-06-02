use std::{env, io::{self, Read, Write}, net::{TcpListener, TcpStream}, thread};

fn main() {
    start_server();
}

fn start_server() {
    let address = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:6969".to_string());

    let listener = TcpListener::bind(&address)
        .expect("Failed to bind server address");
    println!("Server listening on {address}");

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                thread::spawn(move || {
                    handle_client(stream);
                });
            }
            Err(e) => {
                eprintln!("Failed to accept connection: {}", e);
            }
        }
    }
}

fn handle_client(mut stream: TcpStream) {
    let peer_address = stream
        .peer_addr()
        .map_or_else(|_| "unknown".to_string(), |address| address.to_string());
    println!("Handling connection from: {peer_address}");

    let mut buffer = [0; 1024];

    loop {
        match stream.read(&mut buffer) {
            Ok(n) => {
                if n == 0 {
                    println!("Client {peer_address} closed connection");
                    break;
                }

                if let Err(e) = stream.write_all(&buffer[0..n]) {
                    eprintln!("Write error to client {peer_address}: {e}");
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                match e.kind() {
                    io::ErrorKind::ConnectionReset => {
                        println!("Client {peer_address} reset connection");
                    }
                    _ => {
                        eprintln!("Failed to read from client {peer_address}: {e}");
                    }
                }
                break;
            }
        }
    }
    println!("Connection finished for: {peer_address}");
}