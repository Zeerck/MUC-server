use std::{io::Write, net::TcpStream};
use crate::{error, trace};

#[derive(Debug, PartialEq)]
pub enum Command {
    Ping,
    Exit,
}

impl Command {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "/ping" => Some(Command::Ping),
            "/exit" => Some(Command::Exit),
            _ => None
        }
    }
}

pub fn handle_command(stream: TcpStream, command: Command) -> bool {
    match command {
        Command::Ping => {
            match stream.try_clone() {
                Ok(cloned) => ping_handler(cloned),
                Err(e) => error!("Failed to clone stream for PING: {e}"),
            };
            true
        }
        Command::Exit => { 
            match stream.try_clone() {
                Ok(cloned) => exit_handler(cloned),
                Err(e) => error!("Failed to clone stream for EXIT: {e}"),
            };
            false
        }
    }
}

fn ping_handler(mut stream: TcpStream) {
    trace!("Server received PING command");
    let _ = stream.write_all(b"Server: PONG\n");
}

fn exit_handler(mut stream: TcpStream) {
    trace!("Server received EXIT command");
    let _ = stream.write_all(b"Disconnected from server\n");
    if let Err(e) = stream.shutdown(std::net::Shutdown::Both) {
        error!("Failed to shutdown stream: {e}");
    }
}