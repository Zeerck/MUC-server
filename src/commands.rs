use std::{io::Write, net::TcpStream};

use super::logger::prelude::*;

#[derive(Debug, PartialEq)]
pub enum Command {
    Ping,
    Exit,
}

impl Command {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ping" => Some(Command::Ping),
            "exit" => Some(Command::Exit),
            _ => None
        }
    }
}

pub fn handle_command(stream: TcpStream, command: Command) {
    match command {
        Command::Ping => ping_handler(stream.try_clone().expect("Error while clonning stream")),
        Command::Exit => exit_handler(stream.try_clone().expect("Error while clonning stream")),
    }
}

fn ping_handler(mut stream: TcpStream) {
    log("Server received PING command", Trace);
    let _ = stream.write_all("Server: PONG\n".as_bytes());
}

fn exit_handler(stream: TcpStream) {
    log("Server received EXIT command", Trace);
    stream.shutdown(std::net::Shutdown::Both).unwrap();
}