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

/// Start a temporary HTTP server for a local file and return the `http://` URL.
///
/// The server serves from the file's parent directory so that the file (and any
/// sibling assets like CSS/images for HTML) are accessible. The returned
/// [`LocalServer`] **must** be kept alive for as long as the URL is used — the
/// background thread exits when the process does.
pub fn serve_local_file(path: &Path) -> Result<(String, LocalServer)> {
    let abs =
        std::fs::canonicalize(path).map_err(|e| Error::Export(format!("file not found: {e}")))?;
    let parent = abs.parent().unwrap_or(Path::new("/"));
    let filename = abs
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::Export("invalid file name".to_string()))?;
    let server = LocalServer::start(parent)?;
    let url = format!("http://127.0.0.1:{}/{}", server.port(), filename);
    Ok((url, server))
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
    use std::fs;
    use std::io::{BufRead, BufReader, Write};

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

        // Drain all headers so the client doesn't see ERR_ABORTED from an
        // unread request body / headers when we close the connection.
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).is_err() {
                break;
            }
            if header.trim().is_empty() {
                break;
            }
        }

        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let path_str = parts[1].trim_start_matches('/');
        let file_path = root.join(path_str);

        // Security: prevent directory traversal
        if !file_path
            .canonicalize()
            .ok()
            .and_then(|p| root.canonicalize().ok().map(|r| p.starts_with(r)))
            .unwrap_or(false)
        {
            eprintln!("demo: server: 403 forbidden (traversal attempt)");
            if let Ok(mut s) = stream.try_clone() {
                let _ = s.write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            continue;
        }

        match fs::read(&file_path) {
            Ok(content) => {
                let mime = guess_mime(&file_path);
                if let Ok(mut s) = stream.try_clone() {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        mime,
                        content.len()
                    );
                    let _ = s
                        .write_all(header.as_bytes())
                        .and_then(|_| s.write_all(&content));
                }
            }
            Err(e) => {
                eprintln!("demo: server: 404 {path_str}: {e}");
                if let Ok(mut s) = stream.try_clone() {
                    let _ = s.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
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
        assert_eq!(rewritten, "http://127.0.0.1:9999/home/user/doc.pdf");
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
        assert_eq!(guess_mime(Path::new("page.htm")), "text/html");
        assert_eq!(guess_mime(Path::new("style.css")), "text/css");
        assert_eq!(guess_mime(Path::new("app.js")), "application/javascript");
        assert_eq!(guess_mime(Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(guess_mime(Path::new("photo.jpeg")), "image/jpeg");
        assert_eq!(guess_mime(Path::new("animation.gif")), "image/gif");
        assert_eq!(guess_mime(Path::new("icon.svg")), "image/svg+xml");
        assert_eq!(
            guess_mime(Path::new("data.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn mime_type_case_insensitive() {
        assert_eq!(guess_mime(Path::new("DOC.PDF")), "application/pdf");
        assert_eq!(guess_mime(Path::new("IMAGE.PNG")), "image/png");
    }

    #[test]
    fn mime_type_no_extension() {
        assert_eq!(
            guess_mime(Path::new("Makefile")),
            "application/octet-stream"
        );
        assert_eq!(guess_mime(Path::new("README")), "application/octet-stream");
    }

    #[test]
    fn rewrite_url_with_relative_path() {
        let server = LocalServer {
            port: 8080,
            _thread_handle: None,
        };
        let rewritten = server.rewrite_url("file:///tmp/test.html");
        assert_eq!(rewritten, "http://127.0.0.1:8080/tmp/test.html");
    }

    #[test]
    fn serve_local_file_starts_server_and_returns_http_url() {
        let dir = std::env::temp_dir().join(format!("srv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.pdf");
        std::fs::write(&file, b"%PDF-1").unwrap();

        let (url, server) = super::serve_local_file(&file).unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));
        assert!(url.ends_with("/test.pdf"));
        assert!(server.port() > 0);

        // Clean up
        std::fs::remove_file(&file).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn rewrite_url_file_double_slash() {
        let server = LocalServer {
            port: 5555,
            _thread_handle: None,
        };
        let rewritten = server.rewrite_url("file:///a/b/c");
        assert_eq!(rewritten, "http://127.0.0.1:5555/a/b/c");
    }

    #[test]
    fn rewrite_url_file_single_slash() {
        let server = LocalServer {
            port: 7777,
            _thread_handle: None,
        };
        let rewritten = server.rewrite_url("file://a/b");
        assert_eq!(rewritten, "http://127.0.0.1:7777/a/b");
    }

    #[test]
    fn rewrite_url_ftp_unchanged() {
        let server = LocalServer {
            port: 9999,
            _thread_handle: None,
        };
        assert_eq!(
            server.rewrite_url("ftp://example.com/file"),
            "ftp://example.com/file"
        );
    }

    #[test]
    fn rewrite_url_data_unchanged() {
        let server = LocalServer {
            port: 1234,
            _thread_handle: None,
        };
        assert_eq!(
            server.rewrite_url("data:text/html,<h1>Hi</h1>"),
            "data:text/html,<h1>Hi</h1>"
        );
    }

    #[test]
    fn rewrite_url_blob_unchanged() {
        let server = LocalServer {
            port: 4321,
            _thread_handle: None,
        };
        assert_eq!(
            server.rewrite_url("blob:https://example.com/abc"),
            "blob:https://example.com/abc"
        );
    }

    #[test]
    fn rewrite_url_file_root_path() {
        let server = LocalServer {
            port: 8888,
            _thread_handle: None,
        };
        let rewritten = server.rewrite_url("file:///");
        assert_eq!(rewritten, "http://127.0.0.1:8888/");
    }

    #[test]
    fn guess_mime_webp() {
        assert_eq!(
            guess_mime(Path::new("image.webp")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_avif() {
        assert_eq!(
            guess_mime(Path::new("image.avif")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_png() {
        assert_eq!(guess_mime(Path::new("photo.png")), "image/png");
    }

    #[test]
    fn guess_mime_txt() {
        assert_eq!(
            guess_mime(Path::new("readme.txt")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_json() {
        assert_eq!(
            guess_mime(Path::new("data.json")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_xml() {
        assert_eq!(
            guess_mime(Path::new("data.xml")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_mp4() {
        assert_eq!(
            guess_mime(Path::new("video.mp4")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_webm() {
        assert_eq!(
            guess_mime(Path::new("video.webm")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_woff() {
        assert_eq!(
            guess_mime(Path::new("font.woff")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_woff2() {
        assert_eq!(
            guess_mime(Path::new("font.woff2")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_ttf() {
        assert_eq!(
            guess_mime(Path::new("font.ttf")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_otf() {
        assert_eq!(
            guess_mime(Path::new("font.otf")),
            "application/octet-stream"
        );
    }

    #[test]
    fn guess_mime_wasm() {
        assert_eq!(
            guess_mime(Path::new("module.wasm")),
            "application/octet-stream"
        );
    }
}
