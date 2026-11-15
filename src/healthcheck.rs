use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

pub fn check(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok()
}

pub fn run() -> ExitCode {
    let port = match std::env::var("ANQA_PORT") {
        Ok(value) => match value.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                eprintln!("anqa: ANQA_PORT must be a number between 1 and 65535");
                return ExitCode::FAILURE;
            }
        },
        Err(_) => 7409,
    };

    if check(port) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn succeeds_against_a_bound_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(check(port));
    }

    #[test]
    fn fails_on_a_closed_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(!check(port));
    }
}
