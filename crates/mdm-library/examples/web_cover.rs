//! Look up the cover from the internet for each file's album, as the app
//! does for albums without one, and save what is found next to this run.
//!
//! cargo run -p mdm-library --example web_cover -- <files>

use std::path::Path;

use mdm_library::{read_meta, CoverQuery, CoverSource, OnlineCovers};

fn main() {
    let source = OnlineCovers::default();
    for (n, arg) in std::env::args().skip(1).enumerate() {
        let meta = read_meta(Path::new(&arg));
        let Some(query) = CoverQuery::for_track(&meta) else {
            println!("{arg}: too few tags to look up (album and an artist are needed)");
            continue;
        };
        print!("{arg}\n  {query:?}\n  ");
        match source.find(&query) {
            Ok(Some(picture)) => {
                let format = image::guess_format(&picture).map(|f| format!("{f:?}")).unwrap_or_default();
                let size = image::load_from_memory(&picture).map(|i| format!("{}x{}", i.width(), i.height()));
                let out = format!("web-cover-{n}.{}", format.to_lowercase());
                std::fs::write(&out, &picture).expect("write the picture");
                println!("found: {format} {} ({} bytes), saved as {out}", size.unwrap_or_default(), picture.len());
            }
            Ok(None) => println!("none found"),
            Err(e) => println!("lookup failed: {e}"),
        }
    }
}
