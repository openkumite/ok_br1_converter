//! Writes the offline HTML5 gallery (embedded at compile time) next to the exports.

use crate::error::Result;
use serde_json::{Map, Value};
use std::path::Path;

const FILES: &[(&str, &[u8])] = &[
    ("index.html", include_bytes!("../gallery/index.html")),
    ("gallery/style.css", include_bytes!("../gallery/style.css")),
    ("gallery/app.js", include_bytes!("../gallery/app.js")),
    ("gallery/ps1.js", include_bytes!("../gallery/ps1.js")),
    ("gallery/vendor/three.min.js", include_bytes!("../gallery/vendor/three.min.js")),
    ("gallery/vendor/GLTFLoader.js", include_bytes!("../gallery/vendor/GLTFLoader.js")),
    ("gallery/vendor/OrbitControls.js", include_bytes!("../gallery/vendor/OrbitControls.js")),
    ("shaders/godot/ps1.gdshader", include_bytes!("../shaders/godot/ps1.gdshader")),
    ("shaders/glsl/ps1.vert", include_bytes!("../shaders/glsl/ps1.vert")),
    ("shaders/glsl/ps1.frag", include_bytes!("../shaders/glsl/ps1.frag")),
];

pub fn write(out: &Path, manifest: &Map<String, Value>) -> Result<()> {
    for (rel, data) in FILES {
        let p = out.join(rel);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(p, data)?;
    }
    // A script (not JSON fetch) so the page works from file://.
    let js = format!("window.BR1 = {};\n", serde_json::to_string(manifest)?);
    std::fs::write(out.join("manifest.js"), js)?;
    Ok(())
}

/// Wraps a .glb as `models/<name>.glb.js` so the viewer can load it over file://.
pub fn write_glb_js(path: &Path, name: &str, glb: &[u8]) -> Result<()> {
    let b64 = base64(glb);
    std::fs::write(path, format!("window.BR1_GLB = window.BR1_GLB || {{}};\nwindow.BR1_GLB[{name:?}] = \"{b64}\";\n"))?;
    Ok(())
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_rfc4648_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }
}
