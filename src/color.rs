//! Colours: CSS/HTML parsing, HSL, random flat colours, the saved custom palette, and the
//! colour editor's state.

use std::path::PathBuf;

use rune::TextEdit;

use crate::model::{Rgba, Style};

// ---- Conversions --------------------------------------------------------------------------

pub fn to_hex(c: Rgba) -> String {
    let [r, g, b, a] = c.0;
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

/// `h` in degrees, `s` and `l` in 0..=1.
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let u = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [u(r), u(g), u(b)]
}

pub fn rgb_to_hsl([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s.clamp(0.0, 1.0), l)
}

fn parse_hex(h: &str) -> Option<Rgba> {
    let h = h.strip_prefix('#').unwrap_or(h);
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = |i: usize, len: usize| u8::from_str_radix(&h[i..i + len], 16).ok();
    match h.len() {
        3 | 4 => {
            let d = |i| n(i, 1).map(|v| v * 17);
            Some(Rgba([d(0)?, d(1)?, d(2)?, if h.len() == 4 { d(3)? } else { 255 }]))
        }
        6 | 8 => Some(Rgba([n(0, 2)?, n(2, 2)?, n(4, 2)?, if h.len() == 8 { n(6, 2)? } else { 255 }])),
        _ => None,
    }
}

/// The numbers inside `name(...)`, split on commas, spaces and slashes.
fn args<'a>(s: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let inner = s.strip_prefix(name)?.trim_start().strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|p| !p.is_empty()).collect())
}

fn alpha(p: Option<&&str>) -> Option<u8> {
    let Some(p) = p else { return Some(255) };
    let v = match p.strip_suffix('%') {
        Some(pct) => pct.parse::<f32>().ok()? / 100.0,
        None => p.parse::<f32>().ok()?,
    };
    Some((v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Parses an HTML/CSS colour: `#RGB`, `#RRGGBB(AA)`, `rgb()`/`rgba()`, `hsl()`/`hsla()` or a
/// CSS colour name such as `tomato`.
pub fn parse(input: &str) -> Option<Rgba> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some(c) = parse_hex(&s).filter(|_| s.starts_with('#') || s.len() == 6 || s.len() == 8) {
        return Some(c);
    }
    if let Some(a) = args(&s, "rgba").or_else(|| args(&s, "rgb")) {
        let ch = |p: &str| -> Option<u8> {
            match p.strip_suffix('%') {
                Some(pct) => Some((pct.parse::<f32>().ok()? * 2.55).round().clamp(0.0, 255.0) as u8),
                None => Some(p.parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8),
            }
        };
        if a.len() < 3 {
            return None;
        }
        return Some(Rgba([ch(a[0])?, ch(a[1])?, ch(a[2])?, alpha(a.get(3))?]));
    }
    if let Some(a) = args(&s, "hsla").or_else(|| args(&s, "hsl")) {
        if a.len() < 3 {
            return None;
        }
        let h = a[0].trim_end_matches("deg").parse::<f32>().ok()?;
        let pct = |p: &str| -> Option<f32> { Some(p.trim_end_matches('%').parse::<f32>().ok()? / 100.0) };
        let [r, g, b] = hsl_to_rgb(h, pct(a[1])?.clamp(0.0, 1.0), pct(a[2])?.clamp(0.0, 1.0));
        return Some(Rgba([r, g, b, alpha(a.get(3))?]));
    }
    CSS_NAMES.iter().find(|(n, _)| *n == s).and_then(|(_, hex)| parse_hex(hex))
}

// ---- Random flat colours ------------------------------------------------------------------

fn seed() -> u64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9);
    t ^ (t >> 17) ^ 0x2545_F491_4F6C_DD1D
}

/// `n` flat UI colours: saturated but soft, mid-light, their hues spread by the golden angle
/// from a random start so a batch never clusters.
pub fn random_flat(n: usize) -> Vec<Rgba> {
    let mut x = seed() | 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 11) as f32 / (1u64 << 53) as f32
    };
    let start = next() * 360.0;
    (0..n)
        .map(|i| {
            let h = start + i as f32 * 137.508;
            let s = 0.55 + next() * 0.25;
            let l = 0.47 + next() * 0.13;
            let [r, g, b] = hsl_to_rgb(h, s, l);
            Rgba([r, g, b, 255])
        })
        .collect()
}

// ---- Saved palette ------------------------------------------------------------------------

fn palette_path() -> Option<PathBuf> {
    rune::dirs::AppDirs::new("layr").map(|d| d.config().join("palette.txt"))
}

pub fn load_palette() -> Vec<Rgba> {
    // The app was called Overlay Studio before: read its palette until Layr saves its own.
    let old = || rune::dirs::AppDirs::new("overlay-studio").map(|d| d.config().join("palette.txt"));
    let read = |p: PathBuf| std::fs::read_to_string(p).ok();
    let Some(text) = palette_path().and_then(read).or_else(|| old().and_then(read)) else { return Vec::new() };
    text.lines().filter_map(parse_hex).collect()
}

pub fn save_palette(colors: &[Rgba]) {
    let Some(path) = palette_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: Vec<String> = colors.iter().map(|c| to_hex(*c)).collect();
    if let Err(e) = std::fs::write(&path, text.join("\n")) {
        eprintln!("layr: couldn't save the palette to {}: {e}", path.display());
    }
}

// ---- The colour editor --------------------------------------------------------------------

/// Which of the selection's colours the editor changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Stroke,
    Fill,
    Text,
    Hatch,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Stroke => "Outline colour",
            Target::Fill => "Fill colour",
            Target::Text => "Text colour",
            Target::Hatch => "Hatch colour",
        }
    }

    pub fn get(self, st: &Style) -> Rgba {
        match self {
            Target::Stroke => st.stroke,
            Target::Fill => st.fill,
            Target::Text => st.text_color,
            Target::Hatch => st.hatch.as_ref().map_or(st.stroke, |h| h.color),
        }
    }

    pub fn set(self, st: &mut Style, c: Rgba) {
        match self {
            Target::Stroke => st.stroke = c,
            Target::Fill => st.fill = c,
            Target::Text => st.text_color = c,
            Target::Hatch => {
                if let Some(h) = &mut st.hatch {
                    h.color = c;
                }
            }
        }
    }
}

pub struct ColorTool {
    pub target: Target,
    pub h: f32,
    pub s: f32,
    pub l: f32,
    pub a: f32,
    /// The text field taking hex, rgb(), hsl() or a colour name.
    pub field: TextEdit,
    /// What the field held when it last matched the colour.
    seen: String,
    pub invalid: bool,
}

impl ColorTool {
    pub fn new(target: Target, c: Rgba) -> ColorTool {
        let (h, s, l) = rgb_to_hsl([c.0[0], c.0[1], c.0[2]]);
        let a = if c.0[3] == 0 { 1.0 } else { c.0[3] as f32 / 255.0 };
        let mut t = ColorTool { target, h, s, l, a, field: TextEdit::new(""), seen: String::new(), invalid: false };
        t.write_field();
        t
    }

    pub fn color(&self) -> Rgba {
        let [r, g, b] = hsl_to_rgb(self.h, self.s, self.l);
        Rgba([r, g, b, (self.a * 255.0).round() as u8])
    }

    pub fn set_color(&mut self, c: Rgba) {
        let (h, s, l) = rgb_to_hsl([c.0[0], c.0[1], c.0[2]]);
        // Greys have no hue: keep the slider where it was.
        if s > 0.0 {
            self.h = h;
        }
        (self.s, self.l, self.a) = (s, l, c.0[3] as f32 / 255.0);
        self.write_field();
    }

    /// Puts the colour's hex in the field (after a slider or swatch changed it).
    pub fn write_field(&mut self) {
        let hex = to_hex(self.color());
        self.field = TextEdit::new(hex.clone());
        self.seen = hex;
        self.invalid = false;
    }

    /// Reads typing in the field into the colour. True when something changed.
    pub fn sync_hex(&mut self) -> bool {
        if self.field.text == self.seen {
            return false;
        }
        self.seen = self.field.text.clone();
        match parse(&self.seen) {
            Some(c) => {
                let (h, s, l) = rgb_to_hsl([c.0[0], c.0[1], c.0[2]]);
                if s > 0.0 {
                    self.h = h;
                }
                (self.s, self.l, self.a, self.invalid) = (s, l, c.0[3] as f32 / 255.0, false);
            }
            None => self.invalid = true,
        }
        true
    }
}

/// The CSS Color Module Level 4 named colours.
pub const CSS_NAMES: &[(&str, &str)] = &[
    ("aliceblue", "f0f8ff"),
    ("antiquewhite", "faebd7"),
    ("aqua", "00ffff"),
    ("aquamarine", "7fffd4"),
    ("azure", "f0ffff"),
    ("beige", "f5f5dc"),
    ("bisque", "ffe4c4"),
    ("black", "000000"),
    ("blanchedalmond", "ffebcd"),
    ("blue", "0000ff"),
    ("blueviolet", "8a2be2"),
    ("brown", "a52a2a"),
    ("burlywood", "deb887"),
    ("cadetblue", "5f9ea0"),
    ("chartreuse", "7fff00"),
    ("chocolate", "d2691e"),
    ("coral", "ff7f50"),
    ("cornflowerblue", "6495ed"),
    ("cornsilk", "fff8dc"),
    ("crimson", "dc143c"),
    ("cyan", "00ffff"),
    ("darkblue", "00008b"),
    ("darkcyan", "008b8b"),
    ("darkgoldenrod", "b8860b"),
    ("darkgray", "a9a9a9"),
    ("darkgreen", "006400"),
    ("darkgrey", "a9a9a9"),
    ("darkkhaki", "bdb76b"),
    ("darkmagenta", "8b008b"),
    ("darkolivegreen", "556b2f"),
    ("darkorange", "ff8c00"),
    ("darkorchid", "9932cc"),
    ("darkred", "8b0000"),
    ("darksalmon", "e9967a"),
    ("darkseagreen", "8fbc8f"),
    ("darkslateblue", "483d8b"),
    ("darkslategray", "2f4f4f"),
    ("darkslategrey", "2f4f4f"),
    ("darkturquoise", "00ced1"),
    ("darkviolet", "9400d3"),
    ("deeppink", "ff1493"),
    ("deepskyblue", "00bfff"),
    ("dimgray", "696969"),
    ("dimgrey", "696969"),
    ("dodgerblue", "1e90ff"),
    ("firebrick", "b22222"),
    ("floralwhite", "fffaf0"),
    ("forestgreen", "228b22"),
    ("fuchsia", "ff00ff"),
    ("gainsboro", "dcdcdc"),
    ("ghostwhite", "f8f8ff"),
    ("gold", "ffd700"),
    ("goldenrod", "daa520"),
    ("gray", "808080"),
    ("green", "008000"),
    ("greenyellow", "adff2f"),
    ("grey", "808080"),
    ("honeydew", "f0fff0"),
    ("hotpink", "ff69b4"),
    ("indianred", "cd5c5c"),
    ("indigo", "4b0082"),
    ("ivory", "fffff0"),
    ("khaki", "f0e68c"),
    ("lavender", "e6e6fa"),
    ("lavenderblush", "fff0f5"),
    ("lawngreen", "7cfc00"),
    ("lemonchiffon", "fffacd"),
    ("lightblue", "add8e6"),
    ("lightcoral", "f08080"),
    ("lightcyan", "e0ffff"),
    ("lightgoldenrodyellow", "fafad2"),
    ("lightgray", "d3d3d3"),
    ("lightgreen", "90ee90"),
    ("lightgrey", "d3d3d3"),
    ("lightpink", "ffb6c1"),
    ("lightsalmon", "ffa07a"),
    ("lightseagreen", "20b2aa"),
    ("lightskyblue", "87cefa"),
    ("lightslategray", "778899"),
    ("lightslategrey", "778899"),
    ("lightsteelblue", "b0c4de"),
    ("lightyellow", "ffffe0"),
    ("lime", "00ff00"),
    ("limegreen", "32cd32"),
    ("linen", "faf0e6"),
    ("magenta", "ff00ff"),
    ("maroon", "800000"),
    ("mediumaquamarine", "66cdaa"),
    ("mediumblue", "0000cd"),
    ("mediumorchid", "ba55d3"),
    ("mediumpurple", "9370db"),
    ("mediumseagreen", "3cb371"),
    ("mediumslateblue", "7b68ee"),
    ("mediumspringgreen", "00fa9a"),
    ("mediumturquoise", "48d1cc"),
    ("mediumvioletred", "c71585"),
    ("midnightblue", "191970"),
    ("mintcream", "f5fffa"),
    ("mistyrose", "ffe4e1"),
    ("moccasin", "ffe4b5"),
    ("navajowhite", "ffdead"),
    ("navy", "000080"),
    ("oldlace", "fdf5e6"),
    ("olive", "808000"),
    ("olivedrab", "6b8e23"),
    ("orange", "ffa500"),
    ("orangered", "ff4500"),
    ("orchid", "da70d6"),
    ("palegoldenrod", "eee8aa"),
    ("palegreen", "98fb98"),
    ("paleturquoise", "afeeee"),
    ("palevioletred", "db7093"),
    ("papayawhip", "ffefd5"),
    ("peachpuff", "ffdab9"),
    ("peru", "cd853f"),
    ("pink", "ffc0cb"),
    ("plum", "dda0dd"),
    ("powderblue", "b0e0e6"),
    ("purple", "800080"),
    ("rebeccapurple", "663399"),
    ("red", "ff0000"),
    ("rosybrown", "bc8f8f"),
    ("royalblue", "4169e1"),
    ("saddlebrown", "8b4513"),
    ("salmon", "fa8072"),
    ("sandybrown", "f4a460"),
    ("seagreen", "2e8b57"),
    ("seashell", "fff5ee"),
    ("sienna", "a0522d"),
    ("silver", "c0c0c0"),
    ("skyblue", "87ceeb"),
    ("slateblue", "6a5acd"),
    ("slategray", "708090"),
    ("slategrey", "708090"),
    ("snow", "fffafa"),
    ("springgreen", "00ff7f"),
    ("steelblue", "4682b4"),
    ("tan", "d2b48c"),
    ("teal", "008080"),
    ("thistle", "d8bfd8"),
    ("tomato", "ff6347"),
    ("turquoise", "40e0d0"),
    ("violet", "ee82ee"),
    ("wheat", "f5deb3"),
    ("white", "ffffff"),
    ("whitesmoke", "f5f5f5"),
    ("yellow", "ffff00"),
    ("yellowgreen", "9acd32"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_html_form() {
        assert_eq!(parse("#f00"), Some(Rgba([255, 0, 0, 255])));
        assert_eq!(parse("#00ff0080"), Some(Rgba([0, 255, 0, 128])));
        assert_eq!(parse("rgb(10, 20, 30)"), Some(Rgba([10, 20, 30, 255])));
        assert_eq!(parse("rgba(10 20 30 / 50%)"), Some(Rgba([10, 20, 30, 128])));
        assert_eq!(parse("hsl(0, 100%, 50%)"), Some(Rgba([255, 0, 0, 255])));
        assert_eq!(parse("Tomato"), Some(Rgba([255, 99, 71, 255])));
        assert_eq!(parse("nope"), None);
    }

    #[test]
    fn hsl_round_trips() {
        for c in [[200, 40, 90], [10, 200, 120], [128, 128, 128]] {
            let (h, s, l) = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            for i in 0..3 {
                assert!((back[i] as i32 - c[i] as i32).abs() <= 1, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn random_flat_colours_are_neither_grey_nor_extreme() {
        for c in random_flat(12) {
            let (_, s, l) = rgb_to_hsl([c.0[0], c.0[1], c.0[2]]);
            assert!(s > 0.4 && (0.35..0.7).contains(&l), "{c:?}");
        }
    }
}
