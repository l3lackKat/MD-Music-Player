//! Command-line player for trying the engine without the app.
//!
//! ```text
//! cargo run -p mdm-engine --example play -- --list
//! cargo run -p mdm-engine --example play -- track1.flac track2.dsf
//! cargo run -p mdm-engine --example play -- --device "USB DAC" --shared track.flac
//! ```

use std::path::PathBuf;
use std::time::Duration;

use mdm_engine::{list_devices, Command, Event, OutputMode, Player, WasapiBackend};

fn main() {
    let mut paths = Vec::new();
    let mut mode = OutputMode::Exclusive;
    let mut device_filter = None;
    let mut list = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--shared" => mode = OutputMode::Shared,
            "--device" => device_filter = args.next(),
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    let devices = list_devices().unwrap_or_else(|e| {
        eprintln!("could not list devices: {e}");
        std::process::exit(1);
    });
    if list {
        for d in &devices {
            println!("{} {}", if d.is_default { "*" } else { " " }, d.name);
        }
        return;
    }
    if paths.is_empty() {
        eprintln!("usage: play [--list] [--shared] [--device <name part>] <files...>");
        std::process::exit(2);
    }

    let device_id = device_filter.map(|filter| {
        let needle = filter.to_lowercase();
        match devices.iter().find(|d| d.name.to_lowercase().contains(&needle)) {
            Some(d) => d.id.clone(),
            None => {
                eprintln!("no playback device matches \"{filter}\"");
                std::process::exit(1);
            }
        }
    });

    let player = Player::new(Box::new(WasapiBackend { device_id, mode }));
    let events = player.events();
    player.send(Command::Load { paths, index: 0 });

    let mut failed = false;
    loop {
        match events.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::Track { path, signal_path, .. }) => {
                let s = &signal_path;
                println!("\n{path}");
                println!(
                    "  {} {} Hz, {} ch  ->  {}  [{:?} {}-bit, {} Hz{}]  {}  {}",
                    s.source.codec,
                    s.source.sample_rate,
                    s.source.channels,
                    s.device,
                    s.output.container,
                    s.output.valid_bits,
                    s.output.sample_rate,
                    if s.output.dop { ", DoP" } else { "" },
                    if s.exclusive { "exclusive" } else { "shared" },
                    if s.bit_perfect { "bit-perfect" } else { "not bit-perfect" },
                );
            }
            Ok(Event::Position { ms, duration_ms }) => {
                let total = duration_ms.map(|d| format!("{:.1}", d as f64 / 1000.0));
                print!("\r  {:.1} / {} s   ", ms as f64 / 1000.0, total.as_deref().unwrap_or("?"));
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Ok(Event::Error { message }) => {
                eprintln!("\nerror: {message}");
                failed = true;
            }
            Ok(Event::QueueEnded) => break,
            Ok(Event::State { .. }) => {}
            Err(_) => {
                // No event for five seconds: the player is stopped.
                break;
            }
        }
    }
    println!();
    if failed {
        std::process::exit(1);
    }
}
