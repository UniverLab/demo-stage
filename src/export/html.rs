//! Self-contained HTML target: the cast embedded in a page that plays it with
//! asciinema-player (loaded from a CDN). Ideal for the landing — a terminal
//! demo as a single static file, no video weight.

use super::run::Recording;
use crate::error::Result;

const PLAYER_VERSION: &str = "3.8.0";

/// Wrap a recording in a self-contained HTML player page.
pub fn to_html(rec: &Recording) -> Result<String> {
    let cast = super::cast::to_cast(rec)?;
    // Embed the cast as a base64 data URL so the file carries its own data.
    let b64 = base64_encode(cast.as_bytes());
    let title = html_escape(&rec.title);

    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — demo</title>
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/asciinema-player@{PLAYER_VERSION}/dist/bundle/asciinema-player.css">
<style>
  html,body {{ margin:0; background:#0b0f14; }}
  #player {{ max-width: 100%; }}
</style>
</head>
<body>
<div id="player"></div>
<script src="https://cdn.jsdelivr.net/npm/asciinema-player@{PLAYER_VERSION}/dist/bundle/asciinema-player.min.js"></script>
<script>
  AsciinemaPlayer.create(
    "data:text/plain;base64,{b64}",
    document.getElementById("player"),
    {{ autoPlay: true, loop: true, terminalFontFamily: "monospace" }}
  );
</script>
</body>
</html>
"#
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Minimal, dependency-free base64 (standard alphabet, padded).
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn embeds_cast_and_player() {
        let rec = Recording {
            cols: 80,
            rows: 24,
            title: "demo".into(),
            events: vec![(0.1, "hi".into())],
            duration: 0.6,
        };
        let html = to_html(&rec).unwrap();
        assert!(html.contains("asciinema-player"));
        assert!(html.contains("data:text/plain;base64,"));
    }
}
