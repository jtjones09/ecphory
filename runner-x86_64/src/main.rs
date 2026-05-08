//! Host runner for the x86_64 UEFI Ecphory image. Boots
//! `ecphory-x86_64.img` in qemu-system-x86_64 with OVMF + two virtio-
//! blk drives (boot + persistent storage). Same `--shot` / `--keys`
//! ergonomics as runner-aarch64.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::thread;
use std::time::Duration;

const OVMF_CODE: &str = "/usr/share/OVMF/OVMF_CODE_4M.fd";
const OVMF_VARS_TEMPLATE: &str = "/usr/share/OVMF/OVMF_VARS_4M.fd";

fn main() {
    let img_path = env!("X86_64_IMG_PATH");
    let efi_path = env!("X86_64_EFI_PATH");

    let args: Vec<String> = env::args().collect();
    let prog = &args[0];

    let mut display = false;
    let mut shot: Option<PathBuf> = None;
    let mut shot_delay_secs: u64 = 6;
    let mut keys_after_secs: u64 = 6;
    let mut keys: Option<String> = None;
    let mut reset_storage = false;
    let mut iter = args.iter().skip(1);
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--display" => display = true,
            "--reset-storage" => reset_storage = true,
            "--shot" => {
                let p = iter.next().expect("--shot requires a path");
                shot = Some(PathBuf::from(p));
            }
            "--shot-delay" => {
                shot_delay_secs = iter
                    .next()
                    .expect("--shot-delay requires seconds")
                    .parse()
                    .expect("seconds must be an integer");
            }
            "--keys" => {
                keys = Some(
                    iter.next()
                        .expect("--keys requires a string of monitor sendkey tokens")
                        .clone(),
                );
            }
            "--keys-after" => {
                keys_after_secs = iter
                    .next()
                    .expect("--keys-after requires seconds")
                    .parse()
                    .expect("seconds must be an integer");
            }
            "-h" | "--help" => {
                println!(
                    "Usage: {prog} [--display] [--shot <path.ppm>] [--shot-delay <secs>] [--keys <tokens>] [--keys-after <secs>] [--reset-storage]"
                );
                println!("  --display         enable QEMU GTK window");
                println!("  --shot <path>     after boot, dump framebuffer to PPM and quit");
                println!("  --shot-delay N    seconds to wait before dumping (default 6)");
                println!("  --keys 'a b c'    send monitor sendkey tokens (e.g. 'h i ret')");
                println!("  --keys-after N    seconds to wait before sending keys (default 6)");
                println!("  --reset-storage   delete the persistent storage img");
                println!();
                println!("Image: {img_path}");
                println!("EFI:   {efi_path}");
                exit(0);
            }
            other => {
                eprintln!("unknown arg: {other}");
                exit(2);
            }
        }
    }

    if reset_storage {
        let _ = std::fs::remove_file("target/ecphory-x86_64-storage.img");
    }

    let vars_path = std::env::temp_dir().join(format!(
        "ecphory-x86_64-vars-{}.fd",
        std::process::id()
    ));
    std::fs::copy(OVMF_VARS_TEMPLATE, &vars_path).expect("copy OVMF_VARS");

    let need_monitor = shot.is_some() || keys.is_some();
    let monitor_sock = if need_monitor {
        Some(std::env::temp_dir().join(format!(
            "ecphory-x86_64-mon-{}.sock",
            std::process::id()
        )))
    } else {
        None
    };

    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.arg("-machine").arg("q35");
    cmd.arg("-cpu").arg("max");
    cmd.arg("-m").arg("512M");
    cmd.arg("-no-reboot");

    if let Some(sock) = &monitor_sock {
        cmd.arg("-monitor")
            .arg(format!("unix:{},server,nowait", sock.display()));
        cmd.arg("-serial").arg("stdio");
        cmd.stdout(Stdio::piped());
    } else {
        cmd.arg("-serial").arg("mon:stdio");
    }

    if display {
        cmd.arg("-display").arg("gtk");
    } else {
        cmd.arg("-display").arg("none");
        cmd.arg("-vga").arg("std");
    }

    cmd.arg("-drive")
        .arg(format!("if=pflash,format=raw,file={},readonly=on", OVMF_CODE));
    cmd.arg("-drive")
        .arg(format!("if=pflash,format=raw,file={}", vars_path.display()));

    cmd.arg("-device").arg("qemu-xhci,id=xhci");
    cmd.arg("-device").arg("usb-kbd,bus=xhci.0");

    cmd.arg("-drive")
        .arg(format!("file={},format=raw,if=none,id=hd0", img_path));
    cmd.arg("-device").arg("virtio-blk-pci,drive=hd0");

    let storage_path = std::path::Path::new("target/ecphory-x86_64-storage.img");
    if !storage_path.exists() {
        let f = std::fs::File::create(storage_path).expect("create x86 storage.img");
        f.set_len(16 * 1024 * 1024).expect("size storage.img");
        eprintln!("ecphory-x86_64: created fresh {}", storage_path.display());
    }
    cmd.arg("-drive").arg(format!(
        "file={},format=raw,if=none,id=hd1,media=disk",
        storage_path.display()
    ));
    cmd.arg("-device").arg("virtio-blk-pci,drive=hd1");

    eprintln!(
        "ecphory-x86_64: launching{}{}",
        if shot.is_some() { " +shot" } else { "" },
        if keys.is_some() { " +keys" } else { "" },
    );
    let mut child = cmd.spawn().expect("failed to start qemu-system-x86_64");

    if let Some(sock_path) = monitor_sock.as_ref() {
        let stdout = child.stdout.take().unwrap();
        let serial_thread = thread::spawn(move || {
            let r = BufReader::new(stdout);
            for line in r.lines().take(400) {
                match line {
                    Ok(l) => println!("[serial] {l}"),
                    Err(_) => break,
                }
            }
        });

        for _ in 0..30 {
            if sock_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let mut mon = UnixStream::connect(sock_path).expect("connect monitor");
        mon.set_read_timeout(Some(Duration::from_secs(5))).ok();
        thread::sleep(Duration::from_millis(200));

        if let Some(key_string) = keys.as_ref() {
            thread::sleep(Duration::from_secs(keys_after_secs));
            for token in key_string.split_whitespace() {
                writeln!(mon, "sendkey {}", token).unwrap();
                thread::sleep(Duration::from_millis(60));
            }
        }

        if let Some(shot_path) = shot.as_ref() {
            thread::sleep(Duration::from_secs(shot_delay_secs));
            let abs_shot = std::fs::canonicalize(
                shot_path.parent().unwrap_or(Path::new(".")),
            )
            .unwrap_or_else(|_| std::env::current_dir().unwrap())
            .join(shot_path.file_name().unwrap());
            writeln!(mon, "screendump {}", abs_shot.display()).unwrap();
            thread::sleep(Duration::from_millis(1500));
            writeln!(mon, "quit").unwrap();
            let _ = serial_thread.join();
            let _ = child.wait();
            let _ = std::fs::remove_file(sock_path);
            let _ = std::fs::remove_file(&vars_path);
            if abs_shot.exists() {
                eprintln!("ecphory-x86_64: screenshot saved to {}", abs_shot.display());
            } else {
                eprintln!(
                    "ecphory-x86_64: WARNING screenshot not found at {}",
                    abs_shot.display()
                );
                exit(3);
            }
            return;
        }

        thread::sleep(Duration::from_secs(2));
        writeln!(mon, "quit").ok();
        let _ = serial_thread.join();
        let _ = child.wait();
        let _ = std::fs::remove_file(sock_path);
        let _ = std::fs::remove_file(&vars_path);
        return;
    }

    let status = child.wait().expect("failed to wait on qemu");
    let _ = std::fs::remove_file(&vars_path);
    exit(status.code().unwrap_or(1));
}
