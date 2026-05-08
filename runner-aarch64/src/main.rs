//! Host runner for the aarch64 Ecphory image. Boots `ecphory-aarch64.img`
//! in qemu-system-aarch64 with AAVMF (UEFI for ARM). Modeled on the
//! x86_64 runner; same `--shot` and `--display` ergonomics.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::thread;
use std::time::Duration;

const AAVMF_CODE: &str = "/usr/share/AAVMF/AAVMF_CODE.fd";
const AAVMF_VARS_TEMPLATE: &str = "/usr/share/AAVMF/AAVMF_VARS.fd";

fn main() {
    let img_path = env!("AARCH64_IMG_PATH");
    let efi_path = env!("AARCH64_EFI_PATH");

    let args: Vec<String> = env::args().collect();
    let prog = &args[0];

    let mut display = false;
    let mut shot: Option<PathBuf> = None;
    let mut shot_delay_secs: u64 = 18; // TCG aarch64 boot is slow
    let mut keys_after_secs: u64 = 18;
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
                    "Usage: {prog} [--display] [--shot <path.ppm>] [--shot-delay <secs>] [--keys <tokens>] [--keys-after <secs>]"
                );
                println!("  --display         enable QEMU GTK window");
                println!("  --shot <path>     after boot, dump framebuffer to PPM and quit");
                println!("  --shot-delay N    seconds to wait before dumping (default 18)");
                println!("  --keys 'a b c'    send monitor sendkey tokens (e.g. 'h i ret')");
                println!("  --keys-after N    seconds to wait before sending keys (default 18)");
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
        let _ = std::fs::remove_file("target/ecphory-aarch64-storage.img");
    }

    // Each run gets its own AAVMF_VARS so boot order and stale state don't
    // leak between invocations.
    let vars_path = std::env::temp_dir().join(format!(
        "ecphory-aarch64-vars-{}.fd",
        std::process::id()
    ));
    std::fs::copy(AAVMF_VARS_TEMPLATE, &vars_path).expect("copy AAVMF_VARS");

    let need_monitor = shot.is_some() || keys.is_some();
    let monitor_sock = if need_monitor {
        Some(std::env::temp_dir().join(format!(
            "ecphory-aarch64-mon-{}.sock",
            std::process::id()
        )))
    } else {
        None
    };

    let mut cmd = Command::new("qemu-system-aarch64");
    cmd.arg("-M").arg("virt");
    cmd.arg("-cpu").arg("cortex-a72");
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
    }

    cmd.arg("-drive")
        .arg(format!("if=pflash,format=raw,file={},readonly=on", AAVMF_CODE));
    cmd.arg("-drive")
        .arg(format!("if=pflash,format=raw,file={}", vars_path.display()));
    // virtio-gpu-pci is what AAVMF drives. Our kernel renders into a
    // heap buffer and pushes it via GOP Blt — works regardless of mode.
    cmd.arg("-device").arg("virtio-gpu-pci");
    // USB keyboard so QEMU monitor's `sendkey` can drive input headlessly.
    cmd.arg("-device").arg("qemu-xhci,id=xhci");
    cmd.arg("-device").arg("usb-kbd,bus=xhci.0");
    // Boot drive: the kernel's UEFI image (read-only).
    cmd.arg("-drive")
        .arg(format!("file={},format=raw,if=none,id=hd0", img_path));
    cmd.arg("-device").arg("virtio-blk-device,drive=hd0");

    // Persistent storage drive — same as x86: a 16 MiB sparse file
    // wired up so the kernel-core storage agent has a controller to
    // observe and the snapshot has somewhere to live.
    let storage_path = std::path::Path::new("target/ecphory-aarch64-storage.img");
    if !storage_path.exists() {
        let f = std::fs::File::create(storage_path).expect("create aarch64 storage.img");
        f.set_len(16 * 1024 * 1024).expect("size storage.img");
        eprintln!("ecphory-aarch64: created fresh {}", storage_path.display());
    }
    cmd.arg("-drive")
        .arg(format!(
            "file={},format=raw,if=none,id=hd1,media=disk",
            storage_path.display()
        ));
    cmd.arg("-device").arg("virtio-blk-device,drive=hd1");

    eprintln!(
        "ecphory-aarch64: launching{}",
        if shot.is_some() { " +shot" } else { "" }
    );
    let mut child = cmd.spawn().expect("failed to start qemu-system-aarch64");

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
                eprintln!("ecphory-aarch64: screenshot saved to {}", abs_shot.display());
            } else {
                eprintln!(
                    "ecphory-aarch64: WARNING screenshot not found at {}",
                    abs_shot.display()
                );
                exit(3);
            }
            return;
        }

        // Keys-only mode: keep running for keys + grace, then quit.
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
