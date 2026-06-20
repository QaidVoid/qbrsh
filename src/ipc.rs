//! Unix-socket JSON-RPC control interface.
//!
//! A listener thread accepts newline-delimited JSON-RPC requests on a socket in
//! the runtime directory and forwards parsed commands to the dispatch loop via
//! the mailbox (which is `Send`); it never touches `State`. On startup, a URL
//! argument is forwarded to a running instance over the same socket instead of
//! starting a second browser.
//!
//! Request shape: `{"method": "run_command"|"open_url", "params": {...}}`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread;

use crate::core::command::{self, Command, OpenTarget};
use crate::core::msg::Msg;
use crate::core::runtime::Mailbox;

/// The control socket path under the per-user runtime directory
/// (`$XDG_RUNTIME_DIR/qbrsh/`), falling back to the user's own data directory.
/// It never uses a world-accessible location such as `/tmp`.
pub fn socket_path() -> PathBuf {
    let dirs = directories::ProjectDirs::from("", "", "qbrsh");
    let base = dirs
        .as_ref()
        .and_then(|d| d.runtime_dir().map(std::path::Path::to_path_buf))
        .or_else(|| dirs.as_ref().map(|d| d.data_local_dir().to_path_buf()))
        .unwrap_or_else(|| PathBuf::from(".qbrsh"));
    base.join("ipc.sock")
}

/// Parse a JSON-RPC request line into a [`Command`]. Pure and testable.
pub fn parse_request(line: &str) -> Result<Command, String> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let method = value
        .get("method")
        .and_then(|m| m.as_str())
        .ok_or("missing method")?;
    let params = value.get("params");
    match method {
        "run_command" => {
            let command = params
                .and_then(|p| p.get("command"))
                .and_then(|c| c.as_str())
                .ok_or("run_command needs params.command")?;
            Command::parse(command)
        }
        "open_url" => {
            let url = params
                .and_then(|p| p.get("url"))
                .and_then(|u| u.as_str())
                .ok_or("open_url needs params.url")?;
            Ok(Command::Open {
                target: OpenTarget::Tab,
                input: url.to_string(),
            })
        }
        other => Err(format!("unknown method: {other}")),
    }
}

/// Bind the control socket and spawn a listener thread that forwards parsed
/// commands to `mailbox`. A stale socket (no live listener) is removed first.
pub fn serve(mailbox: Mailbox) {
    let path = socket_path();
    let Some(dir) = path.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("[qbrsh] ipc: cannot create {}: {e}", dir.display());
        return;
    }
    // Owner-only directory so no other local user can reach the socket.
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let dir_uid = std::fs::metadata(dir).map(|m| m.uid()).ok();

    // Only touch an existing socket if it sits in our own directory and is owned
    // by us; otherwise refuse rather than risk hijacking another user's path.
    if path.exists() {
        let owned = dir_uid.is_some() && std::fs::metadata(&path).map(|m| m.uid()).ok() == dir_uid;
        if !owned {
            eprintln!(
                "[qbrsh] ipc: refusing socket not owned by current user: {}",
                path.display()
            );
            return;
        }
        if UnixStream::connect(&path).is_err() {
            let _ = std::fs::remove_file(&path);
        }
    }

    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[qbrsh] ipc: cannot bind {}: {e}", path.display());
            return;
        }
    };
    // Owner-only socket.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            handle_client(stream, &mailbox);
        }
    });
}

fn handle_client(stream: UnixStream, mailbox: &Mailbox) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(read_half);
    let mut writer = stream;
    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let response = match parse_request(&line) {
            Ok(cmd) if command::is_remote_safe(&cmd) => {
                mailbox.send(Msg::Command(cmd));
                serde_json::json!({ "ok": true })
            }
            Ok(_) => serde_json::json!({ "error": "command not permitted over ipc" }),
            Err(error) => serde_json::json!({ "error": error }),
        };
        let _ = writeln!(writer, "{response}");
    }
}

/// Try to forward a URL to a running instance over the socket. Returns true if
/// it was delivered (so the caller can exit without starting a second browser).
pub fn forward_url(url: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return false;
    };
    let request = serde_json::json!({ "method": "open_url", "params": { "url": url } });
    if writeln!(stream, "{request}").is_err() {
        return false;
    }
    let _ = stream.flush();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_command() {
        let cmd =
            parse_request(r#"{"method":"run_command","params":{"command":"tabopen https://x"}}"#)
                .unwrap();
        assert!(matches!(
            cmd,
            Command::Open {
                target: OpenTarget::Tab,
                ..
            }
        ));
    }

    #[test]
    fn parses_open_url() {
        let cmd =
            parse_request(r#"{"method":"open_url","params":{"url":"https://a.test"}}"#).unwrap();
        assert_eq!(
            cmd,
            Command::Open {
                target: OpenTarget::Tab,
                input: "https://a.test".to_string()
            }
        );
    }

    #[test]
    fn rejects_unknown_method() {
        assert!(parse_request(r#"{"method":"nope"}"#).is_err());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_request("not json at all").is_err());
    }

    #[test]
    fn rejects_missing_params() {
        assert!(parse_request(r#"{"method":"open_url"}"#).is_err());
    }
}
