//! The web application, embedded in the executable. Setting `CURUST_WEB_DIR` serves the files
//! from that folder instead (for development).

pub fn asset(path: &str) -> Option<(&'static str, Vec<u8>)> {
    let (name, ct, embedded): (&str, &'static str, &'static [u8]) = match path {
        "/" | "/index.html" => ("index.html", "text/html; charset=utf-8", include_bytes!("../web/index.html")),
        "/style.css" => ("style.css", "text/css; charset=utf-8", include_bytes!("../web/style.css")),
        "/app.js" => ("app.js", "application/javascript; charset=utf-8", include_bytes!("../web/app.js")),
        "/viewer.js" => ("viewer.js", "application/javascript; charset=utf-8", include_bytes!("../web/viewer.js")),
        _ => return None,
    };
    if let Ok(dir) = std::env::var("CURUST_WEB_DIR") {
        if let Ok(data) = std::fs::read(std::path::Path::new(&dir).join(name)) {
            return Some((ct, data));
        }
    }
    Some((ct, embedded.to_vec()))
}
