//! Per-user Unix domain socket bridging local CLI/MCP requests into the GUI
//! event loop. Does not process external requests in a worker thread.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

use tr_control::{Command, MAX_MESSAGE_BYTES, Reply, socket_path};

pub struct Request {
    pub command: Command,
    pub response: SyncSender<Reply>,
}

pub struct IpcReceiver {
    pub rx: Receiver<Request>,
    socket_path: std::path::PathBuf,
}

impl IpcReceiver {
    pub fn bind(ctx: egui::Context) -> std::io::Result<Self> {
        let path = socket_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The parent may already hold the user's layouts. Only the new socket
        // is private; never chmod or delete an existing user directory.
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if !meta.file_type().is_socket() {
                return Err(std::io::Error::other("refusing to replace non-socket file"));
            }
            if UnixStream::connect(&path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "another Terminal app is already controlling this socket",
                ));
            }
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("terminal-local-control".into()).spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(mut stream) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(6)));
                        let mut bytes = Vec::new();
                        let reply = match Read::by_ref(&mut stream)
                            .take((MAX_MESSAGE_BYTES + 1) as u64)
                            .read_to_end(&mut bytes)
                        {
                            Ok(_) if bytes.len() <= MAX_MESSAGE_BYTES => {
                                match serde_json::from_slice::<Command>(&bytes) {
                                    Ok(command) => {
                                        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
                                        if tx.send(Request { command, response: reply_tx }).is_err() {
                                            Reply::failure("Terminal GUI is not available")
                                        } else {
                                            ctx.request_repaint();
                                            reply_rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| {
                                                Reply::failure("Terminal UI did not process request")
                                            })
                                        }
                                    }
                                    Err(e) => Reply::failure(format!("invalid command: {e}")),
                                }
                            }
                            Ok(_) => Reply::failure("request too large"),
                            Err(e) => Reply::failure(format!("could not read request: {e}")),
                        };
                        if let Ok(payload) = serde_json::to_vec(&reply) {
                            let _ = stream.write_all(&payload);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Terminal local control connection failed: {e}");
                        break;
                    }
                }
            }
        })?;
        tracing::info!("Terminal local CLI control socket ready");
        Ok(Self { rx, socket_path: path })
    }
}

impl Drop for IpcReceiver {
    fn drop(&mut self) {
        // A newly started app does not delete another app's socket because
        // bind() above refuses a live duplicate in the first place.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
