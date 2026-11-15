use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Join the console pieces back into the one page the console serves.
///
/// `src/console/manifest.txt` lists the pieces in the order the page reads them.
/// An entry is a page, or an index that stands for a folder of pieces, or an
/// order that names the pieces itself. A piece is a run of whole lines that each
/// end in a line break, exactly as they sat in the page. The pieces are read as
/// they are and set down one after the other, so the page comes out byte for byte
/// what it always was.
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("src")
        .join("console");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", root.display());

    let page = slurp(&root.join("page.html"));
    let (head, rest) = page
        .split_once(STYLES_MARK)
        .expect("the page names where the styles go");
    let (mid, tail) = rest
        .split_once(SCRIPT_MARK)
        .expect("the page names where the script goes");

    let mut styles: Vec<String> = Vec::new();
    walk(&root, "styles/index.txt", &mut styles);
    let mut script: Vec<String> = Vec::new();
    walk(&root, "script/order.txt", &mut script);

    // Each marker sits on a line of its own; the newline that ended it belongs to
    // the marker, the pieces carry their own. Drop the marker and its newline.
    let page = format!(
        "{}{}{}{}{}",
        head,
        join(&styles),
        mid.strip_prefix('\n').unwrap_or(mid),
        join(&script),
        tail.strip_prefix('\n').unwrap_or(tail)
    );
    fs::write(out.join("console.html"), page).expect("write the assembled console page");
}

const STYLES_MARK: &str = "<!--erb:styles-->";
const SCRIPT_MARK: &str = "<!--erb:script-->";

/// A piece is taken whole, line breaks and all.
fn slurp(path: &Path) -> String {
    let mut text = String::new();
    fs::File::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .read_to_string(&mut text)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text
}

/// Pieces follow one another with nothing added between them.
fn join(pieces: &[String]) -> String {
    pieces.concat()
}

fn walk(root: &Path, entry: &str, out: &mut Vec<String>) {
    let path = root.join(entry);
    let list = slurp(&path);
    for child in list.lines().filter(|l| !l.trim().is_empty()) {
        if entry.ends_with("index.txt") || entry.ends_with("order.txt") {
            let rel = path
                .parent()
                .unwrap()
                .join(child)
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if entry.ends_with("index.txt") {
                walk(root, &rel, out);
            } else {
                out.push(slurp(&root.join(&rel)));
            }
        } else {
            out.push(slurp(&path));
            return;
        }
    }
}
