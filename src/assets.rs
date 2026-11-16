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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(path: &str) -> String {
        let file = file(path).unwrap_or_else(|| panic!("missing embedded asset {path}"));
        String::from_utf8(file.data.into_owned()).expect("text asset is valid UTF-8")
    }

    #[test]
    fn shell_assets_are_embedded() {
        let index = text("index.html");
        assert!(index.contains("<p class=\"wordmark\">ANQA</p>"));
        assert!(index.contains("id=\"login-form\""));
        assert!(index.contains("id=\"token\""));
        assert!(index.contains("id=\"login-error\""));
        assert!(index.contains("id=\"bot-grid\""));
        assert!(index.contains("id=\"settings-dialog\""));
        assert!(index.contains("id=\"refresh-button\""));
        assert!(index.contains("id=\"settings-button\""));
        assert!(index.contains("id=\"logout-button\""));
        assert!(index.contains("id=\"banner\""));
        assert!(index.contains("id=\"banner-retry\""));
        assert!(index.contains("id=\"empty-state\""));
        assert!(index.contains("id=\"status-announcer\""));
        assert!(index.contains("id=\"main-content\""));

        for path in ["app.css", "theme.js", "app.js", "fonts/OFL.txt"] {
            assert!(file(path).is_some(), "missing embedded asset {path}");
        }
    }

    #[test]
    fn topbar_actions_are_labelled_icons_and_logout_lives_in_settings() {
        let index = text("index.html");
        let topbar = index.split("</header>").next().unwrap_or_default();
        assert!(topbar.contains("aria-label=\"Refresh bots\""));
        assert!(topbar.contains("aria-label=\"Open settings\""));
        assert!(topbar.contains("aria-haspopup=\"dialog\""));
        assert!(topbar.contains("stroke=\"currentColor\""));
        assert!(!topbar.contains("id=\"logout-button\""));

        let settings = index
            .split("id=\"settings-dialog\"")
            .nth(1)
            .unwrap_or_default();
        assert!(settings.contains("id=\"logout-button\""));
    }

    #[test]
    fn font_and_license_are_bundled() {
        let font = file("fonts/JetBrainsMono-Variable.ttf").expect("missing font");
        assert!(font.data.len() > 100_000);
        assert!(!font.data.starts_with(&[0, 0, 0, 0]));

        let license = text("fonts/OFL.txt");
        assert!(license.contains("SIL OPEN FONT LICENSE"));
    }

    #[test]
    fn asset_mime_types_are_correct() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("app.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("theme.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("fonts/OFL.txt"), "text/plain; charset=utf-8");
        assert_eq!(content_type("fonts/JetBrainsMono-Variable.ttf"), "font/ttf");
    }

    #[test]
    fn shell_uses_only_external_scripts_and_styles() {
        let index = text("index.html");
        assert!(!index.contains("<style"));
        assert!(!index.contains(" style="));
        assert!(!index.contains(" onclick="));
        assert!(!index.contains(" onchange="));
        assert!(!index.contains(" onsubmit="));

        let mut scripts = index.split("<script").skip(1);
        assert!(scripts.next().is_some(), "theme.js must be loaded");
        assert!(scripts.next().is_some(), "app.js must be loaded");
        assert!(scripts.next().is_none(), "unexpected third script");
        for tag in index.split("<script").skip(1) {
            let opening = tag.split('>').next().unwrap_or_default();
            assert!(opening.contains("src=\"/assets/"));
        }
    }

    #[test]
    fn theme_script_honours_the_storage_contract() {
        let script = text("theme.js");
        assert!(script.contains("\"anqa.theme\""));
        assert!(script.contains("\"anqa.accent\""));
        assert!(script.contains("prefers-color-scheme"));
        assert!(script.contains("data-theme"));
        assert!(script.contains("data-accent"));
        assert!(script.contains("window.AnqaTheme"));
    }

    #[test]
    fn stylesheet_defines_the_three_themes_and_two_accents() {
        let css = text("app.css");
        assert!(css.contains("@font-face"));
        assert!(css.contains("/assets/fonts/JetBrainsMono-Variable.ttf"));
        assert!(css.contains("format(\"truetype-variations\")"));
        assert!(css.contains("font-weight: 100 800"));
        assert!(css.contains(":root[data-theme=\"dark\"]"));
        assert!(css.contains(":root[data-theme=\"oled\"]"));
        assert!(css.contains(":root[data-accent=\"orange\"]"));
        assert!(css.contains("--primary: #35639c;"));
        assert!(css.contains("--primary: #7daeec;"));
        assert!(css.contains("--primary: #924c2b;"));
        assert!(css.contains("--primary: #e29572;"));
        assert!(css.contains("--topbar-height: 64px"));
        assert!(css.contains("--control-height: 40px"));
        assert!(css.contains("--touch-target: 48px"));
        assert!(!css.contains("box-shadow"));
        assert!(!css.contains("backdrop-filter"));
        assert!(!css.contains("gradient"));
        assert!(!css.contains("left:"));
        assert!(!css.contains("right:"));
        assert!(css.contains("prefers-reduced-motion: reduce"));
        assert!(css.contains(".bot-heading"));
        assert!(css.contains(".status-chip {"));
        assert!(css.contains(".button-icon {"));
        assert!(css.contains(".settings-session-row {"));
    }

    #[test]
    fn app_script_implements_the_spa_contract() {
        let script = text("app.js");
        assert!(script.contains("var POLL_MS = 2000;"));
        assert!(script.contains("fetch("));
        assert!(script.contains("/api/login"));
        assert!(script.contains("/api/logout"));
        assert!(script.contains("/api/bots"));
        assert!(script.contains("/autostart"));
        assert!(script.contains("Content-Type"));
        assert!(script.contains("visibilitychange"));
        assert!(script.contains("aria-busy"));
        assert!(script.contains("formatUptime"));
        assert!(script.contains("textContent"));
        assert!(script.contains("showModal"));
        assert!(!script.contains("innerHTML"));
        assert!(!script.contains("outerHTML"));
        assert!(!script.contains("insertAdjacentHTML"));
        assert!(!script.contains("document.write"));
        assert!(!script.contains("onclick="));
    }

    #[test]
    fn content_type_appends_charsets_to_text_and_leaves_binary_types_alone() {
        assert_eq!(content_type("notes.unknown"), "application/octet-stream");
        assert_eq!(content_type("fonts/JetBrainsMono-Variable.ttf"), "font/ttf");
    }
}
