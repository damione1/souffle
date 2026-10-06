//! Shared text contract: bundled Inter, role styles and build-time projections.
//! No OS-installed fonts or duplicated Slint/Swift style tables.
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const FAMILY: &str = "Inter";
pub const VERSION: &str = "4.1";
pub const REFERENCE_GLYPHS: &str = "& é è ê ë à ç œ Œ É « » ’ … € 0123456789 00:09 00:10 ⌘ ⌥ ⇧";
const SPECIMEN_SIZE: f32 = 24.;

macro_rules! weights {
    ($($variant:ident = $value:literal : $name:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Weight { $($variant),+ }
        impl Weight {
            pub const ALL: &[Self] = &[$(Self::$variant),+];
            pub const fn number(self) -> u16 {
                match self { $(Self::$variant => $value),+ }
            }
            pub const fn slint_name(self) -> &'static str {
                match self { $(Self::$variant => $name),+ }
            }
        }
    };
}
weights!(
    Thin = 100 : "thin",
    ExtraLight = 200 : "extra-light",
    Light = 300 : "light",
    Regular = 400 : "regular",
    Medium = 500 : "medium",
    SemiBold = 600 : "semi-bold",
    Bold = 700 : "bold",
    ExtraBold = 800 : "extra-bold",
    Black = 900 : "black"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slant {
    Upright,
    Italic,
}
impl Slant {
    pub const fn italic(self) -> bool {
        match self {
            Self::Upright => false,
            Self::Italic => true,
        }
    }
}

pub struct FontAsset {
    pub file: &'static str,
    pub postscript: &'static str,
    pub weight: Weight,
    pub slant: Slant,
    pub sha256: &'static str,
    pub bytes: &'static [u8],
}
macro_rules! font {
    ($weight:ident, $slant:ident, $suffix:literal, $sha:literal) => {
        FontAsset {
            file: concat!("Inter-", $suffix, ".ttf"),
            postscript: concat!("Inter-", $suffix),
            weight: Weight::$weight,
            slant: Slant::$slant,
            sha256: $sha,
            bytes: include_bytes!(concat!("../assets/Inter-", $suffix, ".ttf")),
        }
    };
}
pub const FONTS: &[FontAsset] = &[
    font!(
        Thin,
        Upright,
        "Thin",
        "22453f995345e5618d539315a8501f4c74ca41d1898ce44e7d1b8206f0d097d4"
    ),
    font!(
        Thin,
        Italic,
        "ThinItalic",
        "d89ffb2849ef91246283de40a929f671b6e0c8ad1c235701f9362fb0406b4cce"
    ),
    font!(
        ExtraLight,
        Upright,
        "ExtraLight",
        "f3c7079d9eb4a799251a844acea9eb55f99094479306e2ac1bee875dd9f7ae80"
    ),
    font!(
        ExtraLight,
        Italic,
        "ExtraLightItalic",
        "22fca887d9a9336e8a29601b19683947a57b53911c74e251c3657ee798635275"
    ),
    font!(
        Light,
        Upright,
        "Light",
        "164414f0aacbe98a7e64addc43f7b3bfd2e32f7b90e101feeab227f14c371bda"
    ),
    font!(
        Light,
        Italic,
        "LightItalic",
        "c3f9efa776957eefaeac8a2991a990fd1bba6cb928dbaeab7abd0655f3a7693c"
    ),
    font!(
        Regular,
        Upright,
        "Regular",
        "40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82"
    ),
    font!(
        Regular,
        Italic,
        "Italic",
        "bbc051dd204b5019a1aa0bc0ae2aa8a05ab13e7a3f979fa357631dc7feb6833a"
    ),
    font!(
        Medium,
        Upright,
        "Medium",
        "97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872"
    ),
    font!(
        Medium,
        Italic,
        "MediumItalic",
        "51c2c8d7c36f7c26e6e2678b5c3069b329bde9a081154553b0f5bc2d4fc14075"
    ),
    font!(
        SemiBold,
        Upright,
        "SemiBold",
        "78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3"
    ),
    font!(
        SemiBold,
        Italic,
        "SemiBoldItalic",
        "eff2930c3d3b35d3fcf5f76252b6baef4c3e907d9d2fde1d16cf5d417f8deef4"
    ),
    font!(
        Bold,
        Upright,
        "Bold",
        "288316099b1e0a47a4716d159098005eef7c0066921f34e3200393dbdb01947f"
    ),
    font!(
        Bold,
        Italic,
        "BoldItalic",
        "948405a16cdc62701da5f4005ed068ca5f4d27061d98f7974ccfc37831d9581d"
    ),
    font!(
        ExtraBold,
        Upright,
        "ExtraBold",
        "e6756ad5690b77606aa62249a7b420d9902d45cae4b0048a24911fd4324b0a22"
    ),
    font!(
        ExtraBold,
        Italic,
        "ExtraBoldItalic",
        "31b58a00cb9e8d00cb936057285282310e81d2f834461264c543c96983af64a6"
    ),
    font!(
        Black,
        Upright,
        "Black",
        "6342d3ea6dc088b43867f615e807d898adf100c93edb978b8e52c5eb71a264da"
    ),
    font!(
        Black,
        Italic,
        "BlackItalic",
        "1737f7d5b391520e9adcc3ea8c730a0854fe6fdd9b9c93c4100c89beb215a931"
    ),
];

#[derive(Debug, Clone, Copy)]
pub struct TextStyle {
    pub name: &'static str,
    pub size: f32,
    pub weight: Weight,
    pub spacing: f32,
}
impl TextStyle {
    const fn new(name: &'static str, size: f32, weight: Weight, spacing: f32) -> Self {
        Self {
            name,
            size,
            weight,
            spacing,
        }
    }
}
// Role names, not component-owned literals. The half-point sizes preserve
// existing house-control geometry while replacing the family throughout.
pub const BODY: TextStyle = TextStyle::new("body", 14., Weight::Regular, 0.);
pub const HUD_LIVE: TextStyle = TextStyle::new("hud-live", 13., Weight::Regular, 0.);
pub const HUD_LABEL: TextStyle = TextStyle::new("hud-label", 12., Weight::Medium, 0.);
pub const STYLES: &[TextStyle] = &[
    BODY,
    HUD_LIVE,
    HUD_LABEL,
    TextStyle::new("micro", 10., Weight::Regular, 0.),
    TextStyle::new("micro-detail", 10.5, Weight::Regular, 0.),
    TextStyle::new("caption", 11., Weight::Regular, 0.),
    TextStyle::new("caption-label", 11., Weight::Medium, 0.),
    TextStyle::new("caption-emphasis", 11., Weight::SemiBold, 0.),
    TextStyle::new("caption-strong", 11., Weight::Bold, 0.),
    TextStyle::new("small", 11.5, Weight::Regular, 0.),
    TextStyle::new("small-label", 11.5, Weight::Medium, 0.),
    TextStyle::new("small-emphasis", 11.5, Weight::SemiBold, 0.),
    TextStyle::new("secondary", 12., Weight::Regular, 0.),
    TextStyle::new("secondary-emphasis", 12., Weight::SemiBold, 0.),
    TextStyle::new("hint", 12.5, Weight::Regular, 0.),
    TextStyle::new("hint-emphasis", 12.5, Weight::SemiBold, 0.),
    TextStyle::new("content", 13., Weight::Regular, 0.),
    TextStyle::new("label", 13., Weight::Medium, 0.),
    TextStyle::new("section", 13., Weight::SemiBold, 0.),
    TextStyle::new("field-label", 13.5, Weight::Regular, 0.),
    TextStyle::new("field-heading", 13.5, Weight::SemiBold, 0.),
    TextStyle::new("field-strong", 13.5, Weight::Bold, 0.),
    TextStyle::new("body-emphasis", 14., Weight::SemiBold, 0.),
    TextStyle::new("brand", 14.5, Weight::SemiBold, 0.),
    TextStyle::new("action-title", 15., Weight::SemiBold, 0.),
    TextStyle::new("action-body", 15., Weight::Regular, 0.),
    TextStyle::new("dialog-title", 18., Weight::SemiBold, 0.),
    TextStyle::new("dialog-strong", 18., Weight::Bold, 0.),
    TextStyle::new("preview", 19., Weight::Regular, 0.),
    TextStyle::new("settings-title", 20., Weight::Bold, 0.),
    TextStyle::new("title-input", 22., Weight::Regular, 0.),
    TextStyle::new("title", 24., Weight::Bold, 0.),
    TextStyle::new("table-heading", 10.5, Weight::Bold, 1.),
    TextStyle::new("date-heading", 10.5, Weight::SemiBold, 0.6),
    TextStyle::new("timeline-heading", 11., Weight::SemiBold, 1.1),
    TextStyle::new("eyebrow", 11., Weight::SemiBold, 1.65),
];

pub fn font_for(weight: Weight, slant: Slant) -> &'static FontAsset {
    FONTS
        .iter()
        .find(|font| font.weight == weight && font.slant == slant)
        .expect("complete Inter palette")
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn internal_name(bytes: &[u8], id: u16) -> Result<String, String> {
    let face =
        ttf_parser::Face::parse(bytes, 0).map_err(|error| format!("invalid font: {error:?}"))?;
    face.names()
        .into_iter()
        .filter(|name| name.name_id == id)
        .find_map(|name| name.to_string())
        .ok_or_else(|| format!("font name {id} missing"))
}
/// Check the bytes that will be embedded, including required layout glyphs.
pub fn validate_font(asset: &FontAsset, bytes: &[u8]) -> Result<(), String> {
    let fail = |message: String| format!("{}: {message}", asset.file);
    if digest(bytes) != asset.sha256 {
        return Err(fail("SHA-256 mismatch".into()));
    }
    let face = ttf_parser::Face::parse(bytes, 0)
        .map_err(|error| fail(format!("invalid font: {error:?}")))?;
    if internal_name(bytes, ttf_parser::name_id::POST_SCRIPT_NAME)? != asset.postscript {
        return Err(fail(format!(
            "incorrect PostScript name, expected {}",
            asset.postscript
        )));
    }
    let family = internal_name(bytes, ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
        .or_else(|_| internal_name(bytes, ttf_parser::name_id::FAMILY))?;
    if family != FAMILY
        || face.weight().to_number() != asset.weight.number()
        || face.is_italic() != asset.slant.italic()
        || face.is_variable()
    {
        return Err(fail(format!(
            "incorrect family/weight/slant: {family}, {:?}, italic={}",
            face.weight(),
            face.is_italic()
        )));
    }
    for ch in REFERENCE_GLYPHS.chars().filter(|ch| !ch.is_whitespace()) {
        if face.glyph_index(ch).is_none() {
            return Err(fail(format!(
                "missing layout glyph {ch} (U+{:04X})",
                ch as u32
            )));
        }
    }
    Ok(())
}

pub fn asset_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}
pub fn validate_assets(directory: &Path) -> Result<(), String> {
    for font in FONTS {
        let path = directory.join(font.file);
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("{}: required font missing: {error}", path.display()))?;
        validate_font(font, &bytes)?;
    }
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("ttf" | "otf" | "woff" | "woff2" | "ttc")
        ) && !FONTS
            .iter()
            .any(|asset| Some(asset.file) == path.file_name().and_then(|name| name.to_str()))
        {
            return Err(format!("{}: uncontracted font asset", path.display()));
        }
    }
    Ok(())
}

/// Tokenize authored UI without losing line numbers. Comments and strings
/// are distinct tokens, so commented history cannot mask a local binding.
#[derive(Debug)]
struct Token {
    text: String,
    line: usize,
}
fn tokens(source: &str) -> Vec<Token> {
    let mut result = Vec::new();
    let mut chars = source.chars().peekable();
    let mut line = 1;
    while let Some(ch) = chars.next() {
        if ch == '\n' {
            line += 1;
            continue;
        }
        if ch.is_whitespace() {
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for ch in chars.by_ref() {
                if ch == '\n' {
                    line += 1;
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = ' ';
            for ch in chars.by_ref() {
                if ch == '\n' {
                    line += 1;
                }
                if previous == '*' && ch == '/' {
                    break;
                }
                previous = ch;
            }
            continue;
        }
        let start_line = line;
        let mut text = String::from(ch);
        if ch == '"' {
            let mut escaped = false;
            for ch in chars.by_ref() {
                text.push(ch);
                if ch == '\n' {
                    line += 1;
                }
                if ch == '"' && !escaped {
                    break;
                }
                escaped = ch == '\\' && !escaped;
            }
        } else if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            while chars
                .peek()
                .is_some_and(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '-')
            {
                text.push(chars.next().unwrap());
            }
        }
        result.push(Token {
            text,
            line: start_line,
        });
    }
    result
}
fn role_expression(mut values: &[&Token]) -> bool {
    // Peel parentheses enclosing the whole expression.
    loop {
        if values.first().is_none_or(|value| value.text != "(") {
            break;
        }
        let mut depth = 0;
        let close = values.iter().position(|value| {
            if value.text == "(" {
                depth += 1;
            }
            if value.text == ")" {
                depth -= 1;
            }
            depth == 0
        });
        if close == Some(values.len() - 1) {
            values = &values[1..values.len() - 1];
        } else {
            break;
        }
    }
    let mut depth = 0;
    if let Some(question) = values.iter().position(|value| {
        if value.text == "(" {
            depth += 1;
        }
        if value.text == ")" {
            depth -= 1;
        }
        depth == 0 && value.text == "?"
    }) {
        let mut nested = 0;
        let colon = values
            .iter()
            .enumerate()
            .skip(question + 1)
            .find_map(|(i, value)| {
                match value.text.as_str() {
                    "?" => nested += 1,
                    ":" if nested == 0 => return Some(i),
                    ":" => nested -= 1,
                    _ => {}
                }
                None
            });
        return question > 0
            && colon.is_some_and(|colon| {
                role_expression(&values[question + 1..colon])
                    && role_expression(&values[colon + 1..])
            });
    }
    values.len() == 3
        && values[1].text == "."
        && ((values[0].text == "Typography"
            && STYLES.iter().any(|style| style.name == values[2].text))
            || (values[0].text == "root" && values[2].text == "typography"))
}
pub fn validate_slint_source(path: &Path, source: &str) -> Result<(), String> {
    let mut ts = tokens(source);
    // Slint treats underscores and hyphens in identifiers as equivalent.
    // Match the compiler's spelling before validating properties and roles.
    for token in &mut ts {
        if !token.text.starts_with('"') {
            token.text = token.text.replace('_', "-");
        }
    }
    let assigns = |index: usize| {
        ts.get(index).is_some_and(|next| match next.text.as_str() {
            "=" => ts.get(index + 1).is_none_or(|next| next.text != "="),
            "<" => {
                ts.get(index + 1).is_some_and(|next| next.text == "=")
                    && ts.get(index + 2).is_some_and(|next| next.text == ">")
            }
            "+" | "-" | "*" | "/" => ts.get(index + 1).is_some_and(|next| next.text == "="),
            _ => false,
        })
    };
    for (i, token) in ts.iter().enumerate() {
        let text = token.text.as_str();
        let error = |message: &str| format!("{}:{}: {message}", path.display(), token.line);
        // A two-way binding also makes its right-hand property writable. Inspect
        // that side explicitly: an innocently named local alias must not expose
        // an inherited font property or a generated role to later mutations.
        if text == "<"
            && ts.get(i + 1).is_some_and(|next| next.text == "=")
            && ts.get(i + 2).is_some_and(|next| next.text == ">")
        {
            for (offset, value) in ts
                .iter()
                .skip(i + 3)
                .take_while(|next| next.text != ";")
                .enumerate()
            {
                if matches!(
                    value.text.as_str(),
                    "font-family"
                        | "font-size"
                        | "font-weight"
                        | "font-italic"
                        | "letter-spacing"
                        | "typography"
                ) || (value.text == "Typography"
                    && ts.get(i + 4 + offset).is_some_and(|next| next.text == "."))
                {
                    return Err(format!(
                        "{}:{}: two-way aliases cannot expose typography for mutation",
                        path.display(),
                        value.line
                    ));
                }
            }
        }
        if matches!(
            text,
            "Text" | "TextInput" | "TextEdit" | "StyledText" | "LineEdit"
        ) && ts.get(i + 1).is_some_and(|next| next.text == "{")
        {
            return Err(error(
                "raw text item: use AppText/AppTextInput and a Typography role",
            ));
        }
        if matches!(
            text,
            "font-family" | "font-size" | "font-weight" | "font-italic" | "letter-spacing"
        ) && (assigns(i + 1) || ts.get(i + 1).is_some_and(|next| next.text == ":"))
        {
            return Err(error("local typography definition: pass a Typography role"));
        }
        if text == "typography" && assigns(i + 1) {
            return Err(error(
                "typography roles must be bound from the shared contract",
            ));
        }
        if text == "Typography"
            && ts.get(i + 1).is_some_and(|next| next.text == ".")
            && (assigns(i + 3)
                || (ts.get(i + 3).is_some_and(|next| next.text == ".") && assigns(i + 5)))
        {
            return Err(error("the generated Typography contract cannot be mutated"));
        }
        if text == "TypographyStyle"
            && i > 0
            && ts[i - 1].text == "<"
            && ts.get(i + 2).is_some_and(|next| next.text != "typography")
        {
            return Err(error(
                "style inputs must forward the typography role, never a local style object",
            ));
        }
        if text == "typography"
            && ts.get(i + 1).is_some_and(|next| next.text == ".")
            && (assigns(i + 3) || ts.get(i + 3).is_some_and(|next| next.text == ":"))
        {
            return Err(error(
                "a typography role cannot be overridden field by field",
            ));
        }
        if text == "typography" && ts.get(i + 1).is_some_and(|next| next.text == ":") {
            let values: Vec<_> = ts
                .iter()
                .skip(i + 2)
                .take_while(|next| next.text != ";")
                .collect();
            if values.is_empty()
                || values.iter().any(|value| {
                    value.text == "{"
                        || value.text.starts_with('"')
                        || value
                            .text
                            .chars()
                            .next()
                            .is_some_and(|ch| ch.is_ascii_digit())
                })
            {
                return Err(error(
                    "typography must select a shared role, never define local values",
                ));
            }
            if !role_expression(&values) {
                return Err(error("typography must resolve to a named contract role"));
            }
        }
        if matches!(
            text,
            "default-font-family" | "default-font-size" | "default-font-weight"
        ) {
            let expected = match text {
                "default-font-family" => "family",
                "default-font-size" => "size",
                "default-font-weight" => "weight",
                _ => unreachable!("string, not a domain enum"),
            };
            let actual: Vec<_> = ts
                .iter()
                .skip(i + 1)
                .take(7)
                .map(|token| token.text.as_str())
                .collect();
            if actual != [":", "Typography", ".", "body", ".", expected, ";"] {
                return Err(error("window font default must use Typography.body"));
            }
        }
        if text == "Window" && i > 0 && ts[i - 1].text == "inherits" {
            let start = i + 1;
            let mut depth = 0;
            let mut defaults = [false; 3];
            for entry in ts.iter().skip(start) {
                if entry.text == "{" {
                    depth += 1;
                }
                if entry.text == "}" {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                if depth == 1 {
                    match entry.text.as_str() {
                        "default-font-family" => defaults[0] = true,
                        "default-font-size" => defaults[1] = true,
                        "default-font-weight" => defaults[2] = true,
                        _ => {}
                    }
                }
            }
            if !defaults.into_iter().all(|present| present) {
                return Err(error(
                    "window is missing an explicit Typography.body default",
                ));
            }
        }
        if text.starts_with('"')
            && [
                "Archivo",
                "JetBrains",
                "monospace",
                "Helvetica",
                "sans-serif",
                "serif",
            ]
            .iter()
            .any(|legacy| text.contains(legacy))
        {
            return Err(error("legacy or generic text font reference"));
        }
    }
    Ok(())
}
fn source_files(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in
        std::fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?
    {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            source_files(&path, extension, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            files.push(path);
        }
    }
    Ok(())
}
pub fn validate_ui(ui: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    source_files(ui, "slint", &mut files)?;
    files.sort();
    for file in &files {
        validate_slint_source(
            file,
            &std::fs::read_to_string(file).map_err(|error| error.to_string())?,
        )?;
    }
    // Old assets cannot survive outside the contract, even if no import remains.
    fn check_assets(path: &Path) -> Result<(), String> {
        for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                check_assets(&path)?;
            } else if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("ttf" | "otf" | "woff2" | "woff" | "ttc")
            ) {
                return Err(format!(
                    "{}: font asset outside the shared contract",
                    path.display()
                ));
            }
        }
        Ok(())
    }
    check_assets(ui)?;
    Ok(files)
}

pub fn validate_swift(path: &Path) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    validate_swift_source(path, &source)
}
pub fn validate_swift_source(path: &Path, source: &str) -> Result<(), String> {
    let ts = tokens(source);
    for (i, token) in ts.iter().enumerate() {
        let error = || {
            format!(
                "{}:{}: native text must use the generated Typography contract",
                path.display(),
                token.line
            )
        };
        if (token.text == "NSFont"
            && ts
                .get(i + 1)
                .is_some_and(|next| next.text == "." || next.text == "("))
            || token.text.starts_with("CTFontCreate")
            || token.text.starts_with("CTFontManagerRegister")
            || token.text == "NSFontManager"
            || token.text == "withSize"
        {
            return Err(error());
        }
        // The generated factories bind the role when constructing the control,
        // so omitting a later .font assignment cannot enable an AppKit default.
        if matches!(
            token.text.as_str(),
            "NSTextField"
                | "NSSecureTextField"
                | "NSSearchField"
                | "NSComboBox"
                | "NSTextView"
                | "NSText"
                | "NSTextFieldCell"
        ) {
            return Err(error());
        }
        if token.text == "font" && ts.get(i + 1).is_some_and(|next| next.text == "=") {
            if i > 0 && ts[i - 1].text == "." {
                return Err(error());
            }
            let tail = &ts[i + 2..];
            if tail.first().is_none_or(|value| value.text != "Typography")
                || tail.get(1).is_none_or(|value| value.text != ".")
                || tail
                    .get(2)
                    .is_none_or(|value| value.text != "hud_live" && value.text != "hud_label")
                || tail.get(3).is_some_and(|value| {
                    value.text == "." || (value.line == tail[2].line && value.text != ";")
                })
            {
                return Err(error());
            }
        }
    }
    for (index, line) in source.lines().enumerate() {
        let code = line.split("//").next().unwrap_or_default();
        if [
            "NSFont(",
            "NSFont.",
            "systemFont",
            "monospaced",
            "fontWithName",
            "kLiveFontSize",
        ]
        .iter()
        .any(|value| code.contains(value))
        {
            return Err(format!(
                "{}:{}: native text must use the generated Typography contract",
                path.display(),
                index + 1
            ));
        }
    }
    Ok(())
}

pub fn slint_projection() -> String {
    let mut out = String::from("// Generated by souffle-typography. Do not edit.\n");
    for font in FONTS {
        writeln!(out, "import \"{}\";", asset_dir().join(font.file).display()).unwrap();
    }
    out.push_str("export enum TypefaceWeight { ");
    for weight in Weight::ALL {
        write!(out, "{},", weight.slint_name()).unwrap();
    }
    out.push_str("}\nexport struct TypographyStyle { family: string, size: length, weight: int, face-weight: TypefaceWeight, spacing: length, italic: bool }\nexport struct NumericGlyph { text: string, width: length, offset: length }\nexport global Typography {\n    pure callback numeric-cells(string, TypographyStyle) -> [NumericGlyph];\n    pure callback numeric-width(string, TypographyStyle) -> length;\n");
    for style in STYLES {
        writeln!(out, "    out property <TypographyStyle> {}: {{family: \"{FAMILY}\", size: {}px, weight: {}, face-weight: TypefaceWeight.{}, spacing: {}px, italic: false}};", style.name, style.size, style.weight.number(), style.weight.slint_name(), style.spacing).unwrap();
    }
    out.push_str("}\n");
    for (component, builtin) in [("AppText", "Text"), ("AppTextInput", "TextInput")] {
        writeln!(out, "export component {component} inherits {builtin} {{\n    in property <TypographyStyle> typography: Typography.body;\n    font-family: root.typography.family;\n    font-size: root.typography.size;\n    font-weight: root.typography.weight;\n    font-italic: root.typography.italic;\n    letter-spacing: root.typography.spacing;\n}}\n").unwrap();
    }
    out.push_str(r#"
// Fixed digit advances from the actual embedded face. Slint 1.18 does not
// expose OpenType tnum; this retains numeric alignment without another font.
export component AppNumericText inherits Rectangle {
    in property <string> text;
    in property <TypographyStyle> typography: Typography.body;
    in property <brush> text-color;
    in property <TextHorizontalAlignment> horizontal-alignment: left;
    in property <TextVerticalAlignment> vertical-alignment: top;
    private property <length> content-width: Typography.numeric-width(root.text, root.typography);
    preferred-width: root.content-width;
    preferred-height: metric.preferred-height;
    accessible-role: AccessibleRole.text;
    accessible-label: root.text;
    metric := AppText { text: "0"; typography: root.typography; visible: false; }
    for glyph in Typography.numeric-cells(root.text, root.typography): numeric-glyph := AppText {
        x: glyph.offset + (root.horizontal-alignment == TextHorizontalAlignment.right || root.horizontal-alignment == TextHorizontalAlignment.end ? root.width - root.content-width : root.horizontal-alignment == TextHorizontalAlignment.center ? (root.width - root.content-width) / 2 : 0px);
        width: glyph.width;
        height: root.height;
        text: glyph.text;
        typography: root.typography;
        color: root.text-color;
        horizontal-alignment: center;
        vertical-alignment: root.vertical-alignment;
        accessible-role: AccessibleRole.none;
        accessible-label: "";
    }
}
"#);
    // An isolated diagnostic Window, used only by the QA example. It never
    // appears in the product flow and takes its colours from the house theme.
    out.push_str("export struct FontSpecimen { label: string, style: TypographyStyle }\nexport component TypographySpecimen inherits Window {\n    default-font-family: Typography.body.family;\n    default-font-size: Typography.body.size;\n    default-font-weight: Typography.body.weight;\n    in property <brush> foreground;\n    in property <brush> canvas;\n    background: root.canvas;\n    preferred-width: 1040px; preferred-height: 780px;\n    private property <[FontSpecimen]> faces: [\n");
    for font in FONTS {
        writeln!(out, "{{label: \"{} {}\", style: {{family: \"{FAMILY}\", size: {SPECIMEN_SIZE}px, weight: {}, face-weight: TypefaceWeight.{}, spacing: 0px, italic: {}}}}},", font.weight.number(), font.postscript, font.weight.number(), font.weight.slint_name(), font.slant.italic()).unwrap();
    }
    writeln!(out, "];\n    VerticalLayout {{ padding: 18px; alignment: start; spacing: 4px;\n        for face in root.faces: HorizontalLayout {{ spacing: 12px;\n            AppText {{ text: face.label; typography: Typography.caption; color: root.foreground; width: 175px; vertical-alignment: center; }}\n            AppText {{ text: \"{REFERENCE_GLYPHS}\"; typography: face.style; color: root.foreground; horizontal-stretch: 1; }}\n        }}\n    }}\n}}").unwrap();
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct NumericCell {
    pub text: String,
    pub width: f32,
    pub offset: f32,
}
pub fn numeric_cells(text: &str, size: f32, weight: Weight, italic: bool) -> Vec<NumericCell> {
    let asset = FONTS
        .iter()
        .find(|asset| asset.weight == weight && asset.slant.italic() == italic)
        .expect("numeric style must resolve to a contract font");
    let face = ttf_parser::Face::parse(asset.bytes, 0).expect("validated Inter");
    let scale = size / f32::from(face.units_per_em());
    let advance = |ch| {
        face.glyph_index(ch)
            .and_then(|glyph| face.glyph_hor_advance(glyph))
            .map_or(0., f32::from)
            * scale
    };
    let digit = ('0'..='9').map(advance).fold(0., f32::max);
    let mut offset = 0.;
    text.chars()
        .map(|ch| {
            let width = if ch.is_ascii_digit() {
                digit
            } else {
                advance(ch)
            };
            let cell = NumericCell {
                text: ch.to_string(),
                width,
                offset,
            };
            offset += width;
            cell
        })
        .collect()
}

pub fn swift_projection() -> String {
    let mut out = String::from(
        "// Generated by souffle-typography. Do not edit.\nimport AppKit\nimport CoreText\n\nenum Typography {\n    static let names: [String] = [\n",
    );
    for font in FONTS {
        writeln!(out, "        \"{}\",", font.postscript).unwrap();
    }
    out.push_str(
        r#"    ]
    // The exact byte-backed CGFont is retained; neither measuring nor drawing
    // looks up an installed font by a potentially ambiguous family name.
    static var registered: [String: CGFont] = [:]
    static func font(_ name: String, size: CGFloat) -> NSFont {
        guard registered.count == names.count, let cgFont = registered[name] else {
            fatalError("Inter font is not registered: \(name)")
        }
        let ctFont = CTFontCreateWithGraphicsFont(cgFont, size, nil, nil)
        guard CTFontCopyPostScriptName(ctFont) as String == name else {
            fatalError("Inter font resolution failed: \(name)")
        }
        // CTFont / NSFont are toll-free bridged by AppKit.
        return unsafeBitCast(ctFont, to: NSFont.self)
    }
"#,
    );
    writeln!(
        out,
        "    static let referenceGlyphs = \"{REFERENCE_GLYPHS}\""
    )
    .unwrap();
    for style in [HUD_LIVE, HUD_LABEL] {
        let name = style.name.replace('-', "_");
        writeln!(
            out,
            "    static let {name}: NSFont = font(\"{}\", size: {})",
            font_for(style.weight, Slant::Upright).postscript,
            style.size
        )
        .unwrap();
    }
    out.push_str(r#"
    static func makeHUDLabel() -> NSTextField {
        let field = NSTextField(labelWithString: "")
        field.font = hud_label
        return field
    }
    static func makeHUDLiveText() -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: "")
        field.font = hud_live
        return field
    }
}

@_cdecl("pill_typography_register")
func pillTypographyRegister(_ bytes: UnsafePointer<UInt8>?, _ count: Int,
                            _ index: Int32) -> Int32 {
    guard let bytes, count > 0, index >= 0, Int(index) < Typography.names.count,
          let provider = CGDataProvider(data: Data(bytes: bytes, count: count) as CFData),
          let cgFont = CGFont(provider),
          cgFont.postScriptName as String? == Typography.names[Int(index)] else {
        fputs("Inter: invalid font data or internal name\n", stderr)
        return 1
    }
    let name = Typography.names[Int(index)]
    if Typography.registered[name] != nil { return 0 }
    var error: Unmanaged<CFError>?
    guard CTFontManagerRegisterGraphicsFont(cgFont, &error) else {
        fputs("Inter registration failed: \(name): \(String(describing: error?.takeRetainedValue()))\n", stderr)
        return 2
    }
    Typography.registered[name] = cgFont
    return 0
}

// Isolated QA exercises the fonts selected by actual CoreText glyph runs.
@_cdecl("pill_typography_verify")
func pillTypographyVerify() -> Int32 {
    guard Typography.registered.count == Typography.names.count else {
        fputs("Inter: required HUD font is not registered\n", stderr)
        return 1
    }
    for name in Typography.names {
        let font = Typography.font(name, size: Typography.hud_live.pointSize)
        let text = NSAttributedString(string: Typography.referenceGlyphs, attributes: [.font: font])
        let line = CTLineCreateWithAttributedString(text)
        for run in CTLineGetGlyphRuns(line) as! [CTRun] {
            let attributes = CTRunGetAttributes(run) as NSDictionary
            let actual = attributes[kCTFontAttributeName] as! CTFont
            guard CTFontCopyPostScriptName(actual) as String == name else {
                fputs("Inter: HUD glyph run selected a substitute for \(name)\n", stderr)
                return 2
            }
        }
        print("HUD resolved=\(font.fontName) source=embedded-CGFont pointSize=\(font.pointSize) glyphRuns=Inter")
    }
    let tail = "Dernière ligne récente & conservée."
    let text = String(repeating: Typography.referenceGlyphs + " texte descendeur français gypq. ", count: 30) + tail
    let font = Typography.hud_live
    let width = liveTextColumnWidth()
    let kept = lastWrappedLines(text, width: width, maxLines: kMaxLiveLines, font: font)
    guard kept.text.hasSuffix(tail), kept.lines > 0, kept.lines <= kMaxLiveLines else {
        fputs("Inter: HUD tail retention failed\n", stderr)
        return 3
    }
    let measured = (kept.text as NSString).boundingRect(with: NSSize(width: width, height: .greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: [.font: font]).height
    let height = CGFloat(kept.lines) * ceil(font.ascender - font.descender + font.leading) + 2
    guard measured <= height else {
        fputs("Inter: HUD descenders would be clipped\n", stderr)
        return 4
    }
    print("HUD tail retained lines=\(kept.lines) width=\(width) measuredHeight=\(measured) allocatedHeight=\(height)")
    fflush(stdout)
    return 0
}
"#);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_static_face_has_verified_names_weights_and_layout_glyphs() {
        for font in FONTS {
            validate_font(font, font.bytes).unwrap();
        }
        for weight in Weight::ALL {
            for slant in [Slant::Upright, Slant::Italic] {
                assert_eq!(font_for(*weight, slant).weight, *weight);
            }
        }
    }
    #[test]
    fn translated_layout_and_shortcut_glyphs_exist_in_every_static_face() {
        let ui_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../souffle-slint");
        let mut chars = std::collections::BTreeSet::new();
        let mut active = std::collections::BTreeSet::new();
        let mut paths = Vec::new();
        source_files(&ui_root.join("ui"), "slint", &mut paths).unwrap();
        for path in paths {
            for token in tokens(&std::fs::read_to_string(path).unwrap()) {
                if token.text.starts_with('"') {
                    let value = token.text.trim_matches('"').replace("\\\"", "\"");
                    chars.extend(
                        value
                            .chars()
                            .filter(|ch| !ch.is_ascii() && !ch.is_whitespace()),
                    );
                    active.insert(value);
                }
            }
        }
        let mut translated = 0;
        for lang in ["fr", "en"] {
            let source = std::fs::read_to_string(
                ui_root.join(format!("lang/{lang}/LC_MESSAGES/souffle-slint.po")),
            )
            .unwrap();
            // The catalog retains historic entries, including obsolete text icons.
            // Only translations of literals still authored by the current UI belong
            // to the layout glyph contract.
            for entry in source.split("\n\n") {
                let Some(id) = entry.lines().find_map(|line| line.strip_prefix("msgid ")) else {
                    continue;
                };
                let id = id.trim_matches('"').replace("\\\"", "\"");
                if active.contains(&id) {
                    translated += 1;
                    for token in tokens(entry) {
                        if token.text.starts_with('"') {
                            chars.extend(
                                token
                                    .text
                                    .chars()
                                    .filter(|ch| !ch.is_ascii() && !ch.is_whitespace()),
                            );
                        }
                    }
                }
            }
        }
        assert!(
            translated > 100,
            "active translation coverage unexpectedly empty"
        );
        chars.extend("⌘⌥⇧⌃".chars());
        for font in FONTS {
            let face = ttf_parser::Face::parse(font.bytes, 0).unwrap();
            for ch in &chars {
                assert!(
                    face.glyph_index(*ch).is_some(),
                    "{}: layout glyph {ch} U+{:04X} missing",
                    font.file,
                    *ch as u32
                );
            }
        }
    }
    #[test]
    fn negative_fixture_rejects_local_styles_and_uncontracted_roots() {
        for source in [
            "AppText { font-size: 12px; }",
            "AppText { font-family: \"Inter\"; }",
            "AppText { font-weight: Typography.body.weight; }",
            "export component Bad inherits Window {}",
            "Text { text: \"x\"; }",
            "AppText { typography: Typography.body; font-italic: true; }",
            "AppText { typography: {family: \"Arial\", size: 42px}; }",
            "AppText { typography: root.active ? Typography.body : root.local-style; }",
            "property <TypographyStyle> local-style: Typography.body;",
            "root.typography.weight = 700;",
            "root.typography.size += 2px;",
            "clicked => { label.font-size = 42px; }",
            "clicked => { label.font-size += 2px; }",
            "clicked => { label.typography = { family: \"Arial\", size: 42px }; }",
            "clicked => { Typography.body.size = 42px; }",
            "clicked => { Typography.body.size *= 2; }",
            "AppText { font_size: 42px; font_weight: 700; font_family: \"Arial\"; }",
            "AppText { font_weight: 700; }",
            "AppText { font_family: \"Arial\"; }",
            "AppText { font_italic: true; }",
            "AppText { letter_spacing: 1px; }",
            "AppText { font-size: Typography.body.size; font_weight: 700; }",
            "clicked => { label.font_size = 42px; }",
            "Window { default_font_size: 42px; }",
            "Window { default_font-family: \"Arial\"; }",
            "Window { default-font_weight: 700; }",
            "import { TypographyStyle as LocalStyle } from \"@typography\"; property <LocalStyle> local_style: {family: \"Arial\", size: 42px}; AppText { typography <=> local_style; }",
            "AppText { typography <=> root.typography; }",
            "label := AppText {} in-out property <length> local-size <=> label.font-size;",
            "field := AppTextInput {} in-out property <length> local_size <=> field.font_size;",
            "label := AppText {} in-out property <string> local-family <=> label.font_family;",
            "field := AppTextInput {} in-out property <int> local-weight <=> field.font-weight;",
            "label := AppText {} in-out property <bool> local-italic <=> label.font_italic;",
            "field := AppTextInput {} in-out property <length> local-spacing <=> field.letter_spacing;",
            "import { TypographyStyle as LocalStyle } from \"@typography\"; label := AppText {} in-out property <LocalStyle> local-style <=> label.typography;",
            "import { TypographyStyle as LocalStyle } from \"@typography\"; field := AppTextInput {} in-out property <LocalStyle> local_style <=> field.typography;",
            "in-out property <string> local-family <=> Typography.body.family;",
            "in-out property <length> local_size <=> Typography.body_emphasis.size;",
        ] {
            let message = validate_slint_source(
                Path::new("isolated.slint"),
                &format!("// fixture\n{source}"),
            )
            .unwrap_err();
            assert!(message.starts_with("isolated.slint:2:"), "{message}");
        }
    }
    #[test]
    fn comments_and_copy_do_not_bypass_style_detection() {
        assert!(
            validate_slint_source(
                Path::new("ok.slint"),
                "// Text { font-size: 12px; }\nAppText { text: \"font-size: 12px;\"; opacity: self.font-size < 12px || Typography.body.size == 14px ? 1 : 0.5; }"
            )
            .is_ok()
        );
        assert!(
            validate_slint_source(
                Path::new("bad.slint"),
                "AppText { font-size /*x*/ :\n Typography.body.size; }"
            )
            .is_err()
        );
        validate_slint_source(
            Path::new("bindings.slint"),
            "in-out property <string> local-text <=> field.text; field := AppTextInput { text <=> root.text; } AppText { text: \"Typography.body <=> font_size\"; opacity: self.font_size < Typography.body.size ? 1 : 0.5; } in-out property <bool> selected <=> root.active;",
        )
        .unwrap();
    }
    #[test]
    fn native_negative_fixtures_reject_spaced_constructors_and_noncontract_fonts() {
        for source in [
            "modeLabel.font = NSFont (name: \"Arial\", size: 42)",
            "modeLabel.font = NSFont . systemFont(ofSize: 12)",
            "modeLabel.font = CTFontCreateWithName(\"Arial\", 42, nil)",
            "modeLabel.font = localFont",
            "modeLabel.font = Typography.hud_live.withSize(42)",
            "modeLabel.font = Typography.hud_live\n .withSize(42)",
            "let font = Typography.hud_live\n .withSize(42)",
            "private let modeLabel = NSTextField(labelWithString: \"\")",
            "private let newLabel = NSTextField(wrappingLabelWithString: \"\")",
            "private let input = NSSearchField()",
            "let attributes: [NSAttributedString.Key: Any] = [.font: font.withSize(42)]",
            "let resized = NSFontManager.shared.convert(Typography.hud_live, toSize: 42)",
        ] {
            let message = validate_swift_source(
                Path::new("isolated.swift"),
                &format!("// fixture\n{source}"),
            )
            .unwrap_err();
            assert!(message.starts_with("isolated.swift:2:"), "{message}");
        }
        let message = validate_swift_source(
            Path::new("isolated.swift"),
            "// fixture\nlet attributes: [NSAttributedString.Key: Any] = [.font: Typography.hud_live\n .withSize(42)]",
        )
        .unwrap_err();
        assert!(message.starts_with("isolated.swift:3:"), "{message}");
        validate_swift_source(
            Path::new("valid.swift"),
            "let modeLabel = Typography.makeHUDLabel()\nlet liveLabel = Typography.makeHUDLiveText()\nlet font = Typography.hud_live\nlet width = 42\n",
        )
        .unwrap();
    }

    #[test]
    fn slint_identifier_spellings_follow_the_compiler_normalization() {
        validate_slint_source(
            Path::new("valid.slint"),
            "export component Valid inherits Window { default_font-family: Typography.body.family; default-font_size: Typography.body.size; default_font_weight: Typography.body.weight; AppText { typography: Typography.body_emphasis; } }",
        )
        .unwrap();
    }
    #[test]
    fn missing_corrupt_and_misnamed_assets_fail_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            validate_assets(directory.path())
                .unwrap_err()
                .contains("required font missing")
        );
        for font in FONTS {
            std::fs::write(directory.path().join(font.file), font.bytes).unwrap();
        }
        std::fs::remove_file(directory.path().join("Inter-BlackItalic.ttf")).unwrap();
        let message = validate_assets(directory.path()).unwrap_err();
        assert!(
            message.contains("Inter-BlackItalic.ttf") && message.contains("required font missing")
        );
        assert!(
            validate_font(&FONTS[0], b"bad font")
                .unwrap_err()
                .contains("SHA-256 mismatch")
        );
        let mut asset = FontAsset {
            postscript: "Inter-Wrong",
            ..FONTS[0]
        };
        asset.bytes = FONTS[0].bytes;
        assert!(
            validate_font(&asset, asset.bytes)
                .unwrap_err()
                .contains("incorrect PostScript name")
        );
    }
    #[test]
    fn shipped_ui_uses_only_generated_house_text_and_contract_defaults() {
        validate_ui(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../souffle-slint/ui")).unwrap();
        validate_swift(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../swift/pill_panel.swift"))
            .unwrap();
    }
    #[test]
    fn every_counter_digit_slot_keeps_its_position_across_timer_rollover() {
        for style in STYLES {
            let before = numeric_cells("00:09", style.size, style.weight, false);
            let after = numeric_cells("00:10", style.size, style.weight, false);
            assert_eq!(
                before
                    .iter()
                    .map(|cell| (cell.offset, cell.width))
                    .collect::<Vec<_>>(),
                after
                    .iter()
                    .map(|cell| (cell.offset, cell.width))
                    .collect::<Vec<_>>()
            );
        }
    }
}
