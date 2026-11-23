//! Print the measurements of each file's spectrum:
//! `cargo run -p mdm-analyze --example analyze -- <file>...`

use std::path::Path;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: analyze <file>...");
        std::process::exit(2);
    }
    for file in files {
        let name = Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(file.clone());
        match mdm_analyze::analyze(Path::new(&file), |_| true) {
            Ok(s) => println!(
                "{name}: cutoff {}, bits {} of {}, {} {} Hz, {}x{}",
                s.cutoff_hz.map(|hz| format!("{:.1} kHz", hz / 1000.0)).unwrap_or_else(|| "none".into()),
                s.effective_bits.map(|b| b.to_string()).unwrap_or_else(|| "-".into()),
                s.declared_bits.map(|b| b.to_string()).unwrap_or_else(|| "-".into()),
                s.codec,
                s.sample_rate,
                s.width,
                s.height,
            ),
            Err(e) => println!("{name}: {e}"),
        }
    }
}
