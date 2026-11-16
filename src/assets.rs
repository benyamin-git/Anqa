use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/"]
pub struct Assets;

pub fn index() -> Option<rust_embed::EmbeddedFile> {
    Assets::get("index.html")
}

pub fn file(path: &str) -> Option<rust_embed::EmbeddedFile> {
    Assets::get(path)
}

pub fn content_type(path: &str) -> String {
    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();
    if content_type.starts_with("text/") {
        format!("{content_type}; charset=utf-8")
    } else {
        content_type
    }
}
