//! Embedded font registry. Each bundled monospace font is included at compile
//! time and looked up by the `font_family` name stored in the score's layout.

use fontdue::{Font, FontSettings};

/// A bundled font: its human-readable name and raw bytes.
struct BundledFont {
    name: &'static str,
    bytes: &'static [u8],
}

/// All fonts embedded in the binary.
const BUNDLED: &[BundledFont] = &[
    BundledFont {
        name: "DejaVu Sans Mono",
        bytes: include_bytes!("../assets/DejaVuSansMono.ttf"),
    },
    BundledFont {
        name: "JetBrains Mono",
        bytes: include_bytes!("../assets/JetBrainsMono-Regular.ttf"),
    },
    BundledFont {
        name: "IBM Plex Mono",
        bytes: include_bytes!("../assets/IBMPlexMono.ttf"),
    },
    BundledFont {
        name: "Liberation Mono",
        bytes: include_bytes!("../assets/LiberationMono-Regular.ttf"),
    },
    BundledFont {
        name: "Ubuntu Mono",
        bytes: include_bytes!("../assets/UbuntuMono.ttf"),
    },
];

/// Wizard display strings (shown in the capture prompt picker).
pub const FONT_NAMES: &[&str] = &[
    "DejaVu Sans Mono   (best Unicode — MapSCII, box drawing)",
    "JetBrains Mono     (clean, modern, ligatures)",
    "IBM Plex Mono      (UniverLab landing font)",
    "Liberation Mono    (Courier New compatible)",
    "Ubuntu Mono        (compact, classic Ubuntu)",
];

/// Short key names stored in `layout.font_family`.
pub const FONT_KEYS: &[&str] = &[
    "DejaVu Sans Mono",
    "JetBrains Mono",
    "IBM Plex Mono",
    "Liberation Mono",
    "Ubuntu Mono",
];

/// Default font (first entry).
pub const DEFAULT_FONT: &str = "DejaVu Sans Mono";

/// Parse a font key from the wizard display string.
/// "DejaVu Sans Mono   (best Unicode …)" → "DejaVu Sans Mono"
pub fn parse_font_name(display: &str) -> &str {
    display.split("   ").next().unwrap_or(display).trim()
}

/// Load a font by its key name. Falls back to the default if unknown.
pub fn load(name: &str) -> Font {
    let bytes = BUNDLED
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(name))
        .map(|f| f.bytes)
        .unwrap_or(BUNDLED[0].bytes);
    Font::from_bytes(bytes, FontSettings::default()).expect("bundled font failed to parse")
}

/// Load a font by wizard display string.
pub fn load_from_display(display: &str) -> Font {
    load(parse_font_name(display))
}
