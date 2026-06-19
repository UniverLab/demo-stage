//! asciinema v2 cast output — the lightweight, text-only target.
//!
//! Format: a JSON header line, then one `[time, "o", data]` JSON line per
//! output event. <https://docs.asciinema.org/manual/asciicast/v2/>

use serde_json::json;

use super::run::Recording;
use crate::error::Result;

/// Render a recording as an asciinema v2 cast.
pub fn to_cast(rec: &Recording) -> Result<String> {
    let header = json!({
        "version": 2,
        "width": rec.cols,
        "height": rec.rows,
        "title": rec.title,
        "env": { "TERM": "xterm-256color" },
    });

    let mut out = serde_json::to_string(&header)?;
    out.push('\n');
    for (t, data) in &rec.events {
        out.push_str(&serde_json::to_string(&json!([t, "o", data]))?);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_header_and_events() {
        let rec = Recording {
            cols: 80,
            rows: 24,
            title: "t".into(),
            events: vec![(0.1, "hi".into()), (0.2, "\r\n".into())],
            captions: vec![],
            focuses: vec![],
            duration: 0.7,
        };
        let cast = to_cast(&rec).unwrap();
        let lines: Vec<&str> = cast.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"version\":2"));
        assert!(lines[0].contains("\"width\":80"));
        assert!(lines[1].starts_with("[0.1,\"o\",\"hi\"]"));
        // control bytes must be JSON-escaped
        assert!(lines[2].contains("\\r\\n"));
    }
}
