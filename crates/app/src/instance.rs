//! One window per project: a second `rubyscope <file>` (or a link) for a
//! project that is already open sends the file to that window and ends.
//!
//! Each window listens on a Unix socket named after its project folder, in
//! `$XDG_RUNTIME_DIR/rubyscope/` (else a folder of the user in the temp
//! folder). A new start connects to the socket of its project. When nothing
//! answers, it opens its own window as before.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui;
use serde::{Deserialize, Serialize};

/// Only the real app listens. Tests make many windows at once and never
/// turn this on.
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable() {
    ENABLED.store(true, Ordering::SeqCst);
}

/// A file to open, with its 0-based line.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Request {
    pub file: PathBuf,
    pub line: Option<u32>,
}

/// The folder of the sockets, made for this user only.
fn socket_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(run) => PathBuf::from(run).join("rubyscope"),
        None => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("rubyscope-{user}"))
        }
    };
    std::fs::create_dir_all(&dir).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(dir)
}

/// The socket of a project: a hash of its folder (FNV-1a, so every build
/// gives the same name).
fn socket_path(dir: &Path, root: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.as_os_str().as_encoded_bytes() {
        hash = (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    dir.join(format!("{hash:016x}.sock"))
}

/// Sends a file to the open window of `root`. True when that window took it.
#[cfg(unix)]
pub fn send(root: &Path, request: &Request) -> bool {
    if !ENABLED.load(Ordering::SeqCst) {
        return false;
    }
    socket_dir().is_some_and(|dir| send_to(&socket_path(&dir, root), request))
}

#[cfg(unix)]
fn send_to(path: &Path, request: &Request) -> bool {
    use std::os::unix::net::UnixStream;
    let Ok(mut stream) = UnixStream::connect(path) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let Ok(mut line) = serde_json::to_string(request) else { return false };
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() {
        return false;
    }
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).is_ok() && answer.trim() == "ok"
}

#[cfg(not(unix))]
pub fn send(_root: &Path, _request: &Request) -> bool {
    false
}

/// The listening side of a window. Dropping it stops listening and removes
/// the socket.
pub struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Listens for files of `root`. None when another window of the same
    /// project already listens, or the socket cannot be made.
    #[cfg(unix)]
    pub fn start(root: &Path, ctx: egui::Context, tx: mpsc::Sender<Request>) -> Option<Server> {
        if !ENABLED.load(Ordering::SeqCst) {
            return None;
        }
        Server::listen(socket_path(&socket_dir()?, root), ctx, tx)
    }

    #[cfg(unix)]
    fn listen(path: PathBuf, ctx: egui::Context, tx: mpsc::Sender<Request>) -> Option<Server> {
        use std::os::unix::net::{UnixListener, UnixStream};
        if UnixStream::connect(&path).is_ok() {
            return None; // another window of this project
        }
        let _ = std::fs::remove_file(&path); // left by a window that crashed
        let listener = UnixListener::bind(&path).ok()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("rubyscope-instance".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    let request =
                        reader.read_line(&mut line).ok().and_then(|_| serde_json::from_str::<Request>(&line).ok());
                    let answer = match request {
                        Some(r) if r.file.is_absolute() && r.file.is_file() => {
                            let _ = tx.send(r);
                            ctx.request_repaint();
                            "ok\n"
                        }
                        _ => "refused\n",
                    };
                    let _ = (&stream).write_all(answer.as_bytes());
                }
            })
            .ok()?;
        Some(Server { path, stop })
    }

    #[cfg(not(unix))]
    pub fn start(_root: &Path, _ctx: egui::Context, _tx: mpsc::Sender<Request>) -> Option<Server> {
        None
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the thread from `accept`, so it sees `stop` and ends.
        #[cfg(unix)]
        let _ = std::os::unix::net::UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Brings the window to the front after a file came in. On Hyprland a
/// window may not raise itself, so ask Hyprland to focus this process.
pub fn raise(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    if cfg!(target_os = "linux") && std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let _ = std::process::Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &format!("pid:{}", std::process::id())])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn socket_names_are_stable_and_per_project() {
        let dir = Path::new("/run/x");
        let a = socket_path(dir, Path::new("/srv/shop"));
        assert_eq!(a, socket_path(dir, Path::new("/srv/shop")));
        assert_ne!(a, socket_path(dir, Path::new("/srv/shop2")));
        assert!(a.to_string_lossy().ends_with(".sock"));
    }

    #[test]
    fn a_second_start_hands_the_file_to_the_open_window() {
        let dir = std::env::temp_dir().join(format!("rubyscope-instance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("order.rb");
        std::fs::write(&file, "class Order\nend\n").unwrap();
        let path = dir.join("test.sock");
        let (tx, rx) = mpsc::channel();
        let server = Server::listen(path.clone(), egui::Context::default(), tx).expect("listens");
        // A second window of the same project does not listen.
        let (tx2, _rx2) = mpsc::channel();
        assert!(Server::listen(path.clone(), egui::Context::default(), tx2).is_none());

        let request = Request { file: file.clone(), line: Some(11) };
        assert!(send_to(&path, &request));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), request);
        // A file that is not there is refused, and nothing is opened.
        assert!(!send_to(&path, &Request { file: dir.join("gone.rb"), line: None }));
        assert!(rx.try_recv().is_err());

        drop(server);
        assert!(!path.exists());
        assert!(!send_to(&path, &request));
    }
}
