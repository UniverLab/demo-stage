//! Temporary HTTP server for serving local files during browser pane export.
//!
//! When `demo export` needs to render browser panes with local files (PDF, images, HTML),
//! Chromium headless has security restrictions that prevent direct `file://` access.
//! This module spins up a temporary HTTP server to serve files from the filesystem,
//! then rewrites `file://` URLs to `http://127.0.0.1:port/...` for the duration of export.

use std::net::TcpListener;
use std::path::Path;
use std::thread;

use crate::error::{Error, Result};

/// A temporary HTTP server running in a background thread.
pub struct LocalServer {
    port: u16,
    _thread_handle: Option<thread::JoinHandle<()>>,
}

impl LocalServer {
    /// Start a temporary HTTP server on an available port, serving from `root_dir`.
    /// Returns the port number.
    pub fn start(root_dir: &Path) -> Result<Self> {
        let root = root_dir.to_path_buf();
        let port = find_available_port()?;

        let listener = TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| Error::Export(format!("failed to bind server: {e}")))?;

        let handle = thread::spawn(move || {
            serve_http(&listener, &root);
        });

        Ok(LocalServer {
            port,
            _thread_handle: Some(handle),
        })
    }

    /// The port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Rewrite a `file://` URL to use the HTTP server.
    /// Example: `file:///home/user/doc.pdf` → `http://127.0.0.1:8001/home/user/doc.pdf`
    pub fn rewrite_url(&self, url: &str) -> String {
        if let Some(rest) = url.strip_prefix("file:///") {
            format!("http://127.0.0.1:{}/{}", self.port, rest)
        } else if let Some(rest) = url.strip_prefix("file://") {
            format!("http://127.0.0.1:{}/{}", self.port, rest)
        } else {
            url.to_string()
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        // The thread will exit naturally when the listener is dropped
        // (client connections close after requests complete)
    }
}

/// Find an available TCP port.
fn find_available_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| Error::Export(format!("failed to find available port: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| Error::Export(format!("failed to get socket address: {e}")))?;
    Ok(addr.port())
}

/// Simple HTTP server loop: accept requests, serve files.
fn serve_http(listener: &TcpListener, root: &Path) {
    use std::io::{BufRead, BufReader, Write};
    use std::fs;

    for stream in listener.incoming().flatten() {
        let stream_clone = match stream.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        };
        
        let mut reader = BufReader::new(stream_clone);
        let mut request_line = String::new();

        if reader.read_line(&mut request_line).is_err() {
            continue;
        }

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let path_str = parts[1].trim_start_matches('/');
        let file_path = root.join(path_str);

        // Security: prevent directory traversal
        if !file_path.canonicalize()
            .ok()
            .and_then(|p| root.canonicalize().ok().map(|r| p.starts_with(r)))
            .unwrap_or(false)
        {
            if let Ok(mut s) = stream.try_clone() {
                let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
            }
            continue;
        }

        match fs::read(&file_path) {
            Ok(content) => {
                let mime = guess_mime(&file_path);
                if let Ok(mut s) = stream.try_clone() {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                        mime,
                        content.len()
                    );
                    let _ = s.write_all(header.as_bytes()).and_then(|_| s.write_all(&content));
                }
            }
            Err(_) => {
                if let Ok(mut s) = stream.try_clone() {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                }
            }
        }
    }
}

/// Guess MIME type by file extension.
fn guess_mime(path: &Path) -> &'static str {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "pdf" => "application/pdf",
            "html" | "htm" => "text/html",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "css" => "text/css",
            "js" => "application/javascript",
            _ => "application/octet-stream",
        })
        .unwrap_or("application/octet-stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_url_absolute_unix_path() {
        let server = LocalServer {
            port: 9999,
            _thread_handle: None,
        };
        let rewritten = server.rewrite_url("file:///home/user/doc.pdf");
        assert_eq!(
            rewritten,
            "http://127.0.0.1:9999/home/user/doc.pdf"
        );
    }

    #[test]
    fn rewrite_url_leaves_http_unchanged() {
        let server = LocalServer {
            port: 9999,
            _thread_handle: None,
        };
        let url = "http://example.com/page.html";
        assert_eq!(server.rewrite_url(url), url);
    }

    #[test]
    fn mime_type_detection() {
        assert_eq!(guess_mime(Path::new("doc.pdf")), "application/pdf");
        assert_eq!(guess_mime(Path::new("image.png")), "image/png");
        assert_eq!(guess_mime(Path::new("page.html")), "text/html");
    }
}
