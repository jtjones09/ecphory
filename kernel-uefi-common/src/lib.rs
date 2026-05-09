//! Shared UEFI helpers for the per-arch kernels.
//!
//! UEFI is the common substrate for both x86_64 and aarch64 ecphory
//! kernels. The firmware exposes:
//!   - GOP for framebuffer (we render into a heap buffer and Blt-push)
//!   - Simple Text Input for keyboard
//!   - BlockIO for persistent storage
//!   - ACPI configuration tables for the RSDP
//!   - Memory map for region observation
//!
//! Both per-arch binaries share this helpers crate. The arch-specific
//! pieces (CPU register reads, port-I/O PCI on x86) stay in their own
//! crates.

#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use uefi::Status;
use uefi::boot::{MemoryType, ScopedProtocol, SearchType};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::{
    BltOp, BltPixel, BltRegion, GraphicsOutput, PixelFormat as GopPixelFormat,
};
use uefi::proto::console::text::Input;
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::table::cfg::ConfigTableEntry;

use kernel_core::framebuffer::{FbInfo, FrameBufferWriter, PixelFormat};
use kernel_core::ops::{BlockResult, Op, OpResult};
use kernel_core::storage_inventory::{self, BlockDeviceInfo};
use kernel_core::{MemoryKind, MemoryRegion};

// ---------- substrate-wide singletons ----------

static GOP_PTR: AtomicPtr<GraphicsOutput> = AtomicPtr::new(core::ptr::null_mut());
static INPUT_PTR: AtomicPtr<Input> = AtomicPtr::new(core::ptr::null_mut());
static BLOCKIO_PTR: AtomicPtr<BlockIO> = AtomicPtr::new(core::ptr::null_mut());

static FB_BUFFER_PTR: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static FB_BUFFER_LEN: AtomicU64 = AtomicU64::new(0);
static FB_WIDTH: AtomicU64 = AtomicU64::new(0);
static FB_HEIGHT: AtomicU64 = AtomicU64::new(0);
static FB_STRIDE: AtomicU64 = AtomicU64::new(0);

static STORAGE_BLOCK_SIZE: AtomicU64 = AtomicU64::new(0);
static STORAGE_TOTAL_BLOCKS: AtomicU64 = AtomicU64::new(0);

static MONOTONIC_TICKS: AtomicU64 = AtomicU64::new(0);

pub static FB: spin::Mutex<Option<FrameBufferWriter>> = spin::Mutex::new(None);

// ---------- substrate setup ----------

#[derive(Debug, Clone)]
pub struct UefiSubstrate {
    pub fb_info: Option<FbInfo>,
    pub storage_present: bool,
    pub storage_blocks: u64,
    pub storage_block_size: u64,
    pub storage_label: String,
    pub keyboard_present: bool,
}

/// Bring UEFI services online: install the kernel-core heap, attach
/// the framebuffer through GOP, attach the keyboard via Simple Text
/// Input, pick the largest non-boot BlockIO device for storage.
pub fn init() -> UefiSubstrate {
    uefi::helpers::init().expect("uefi::helpers::init");
    kernel_core::heap::init();

    // Framebuffer.
    let mut fb_info = None;
    if let Ok(handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() {
        if let Ok(mut gop) = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle) {
            let _ = pick_largest_mode(&mut gop);
            let mode = gop.current_mode_info();
            let (width, height) = mode.resolution();
            let stride = mode.stride();
            let bytes_per_pixel: usize = 4;
            let buf_len = stride * height * bytes_per_pixel;
            let mut buffer: Vec<u8> = vec![0u8; buf_len];
            let buffer_ptr = buffer.as_mut_ptr();
            let buffer_static: &'static mut [u8] = unsafe {
                core::mem::forget(buffer);
                core::slice::from_raw_parts_mut(buffer_ptr, buf_len)
            };
            let info = FbInfo {
                width,
                height,
                stride,
                bytes_per_pixel,
                pixel_format: PixelFormat::Bgr,
            };
            *FB.lock() = Some(FrameBufferWriter::new(buffer_static, info));

            let raw: *mut GraphicsOutput = (&mut *gop) as *mut GraphicsOutput;
            GOP_PTR.store(raw, Ordering::Release);
            core::mem::forget(gop);

            FB_BUFFER_PTR.store(buffer_ptr, Ordering::Release);
            FB_BUFFER_LEN.store(buf_len as u64, Ordering::Release);
            FB_WIDTH.store(width as u64, Ordering::Release);
            FB_HEIGHT.store(height as u64, Ordering::Release);
            FB_STRIDE.store(stride as u64, Ordering::Release);
            fb_info = Some(info);
        }
    }

    // Keyboard.
    let mut keyboard_present = false;
    if let Ok(handle) = uefi::boot::get_handle_for_protocol::<Input>() {
        if let Ok(mut input) = uefi::boot::open_protocol_exclusive::<Input>(handle) {
            let raw: *mut Input = (&mut *input) as *mut Input;
            INPUT_PTR.store(raw, Ordering::Release);
            core::mem::forget(input);
            keyboard_present = true;
        }
    }

    // Storage: largest non-boot BlockIO device.
    let mut storage_present = false;
    let mut storage_blocks = 0u64;
    let mut storage_block_size = 0u64;
    let mut storage_label = String::new();

    let boot_device_handle = current_image_device_handle();
    let boot_path_bytes: Option<Vec<u8>> = boot_device_handle.and_then(|h| {
        uefi::boot::open_protocol_exclusive::<DevicePath>(h)
            .ok()
            .map(|dp| device_path_bytes(&dp))
    });
    let mut inventory: Vec<BlockDeviceInfo> = Vec::new();
    if let Ok(handles) =
        uefi::boot::locate_handle_buffer(SearchType::from_proto::<BlockIO>())
    {
        // First pass — collect inventory of every BlockIO with its
        // boot/ancestor/partition flags. The kernel renders this list
        // through the `disks` command so the operator can see every
        // device the picker considered before any write happens.
        for (i, handle) in handles.iter().enumerate() {
            let is_boot_device = Some(*handle) == boot_device_handle;
            let mut is_boot_ancestor = false;
            if let Some(boot_bytes) = boot_path_bytes.as_ref() {
                if let Ok(cand_dp) =
                    uefi::boot::open_protocol_exclusive::<DevicePath>(*handle)
                {
                    let cand_bytes = device_path_bytes(&cand_dp);
                    if is_strict_prefix(&cand_bytes, boot_bytes) {
                        is_boot_ancestor = true;
                    }
                }
            }
            let Ok(bio) = uefi::boot::open_protocol_exclusive::<BlockIO>(*handle) else {
                continue;
            };
            let media = bio.media();
            let is_logical_partition = media.is_logical_partition();
            let last_block = media.last_block();
            let block_size = media.block_size() as u64;
            let blocks = if last_block == 0 { 0 } else { last_block + 1 };
            inventory.push(BlockDeviceInfo {
                index: i,
                label: device_label(blocks, block_size, is_logical_partition),
                blocks,
                block_size,
                is_logical_partition,
                is_boot_device,
                is_boot_ancestor,
                picked_for_storage: false,
            });
        }

        // Second pass — pick the largest non-boot, non-ancestor, non-
        // partition device. Same selection rule as before; the inventory
        // pass above is purely additive.
        let mut best: Option<(usize, u64, u64)> = None;
        for (i, dev) in inventory.iter().enumerate() {
            if dev.is_boot_device
                || dev.is_boot_ancestor
                || dev.is_logical_partition
                || dev.block_size == 0
                || dev.blocks == 0
            {
                continue;
            }
            let total = dev.blocks.saturating_mul(dev.block_size);
            let take = match best {
                None => true,
                Some((_, prev_total, _)) => total > prev_total,
            };
            if take {
                best = Some((i, total, dev.block_size));
            }
        }
        if let Some((inv_idx, total, _)) = best {
            let handle_idx = inventory[inv_idx].index;
            if let Ok(mut bio) =
                uefi::boot::open_protocol_exclusive::<BlockIO>(handles[handle_idx])
            {
                let raw: *mut BlockIO = (&mut *bio) as *mut BlockIO;
                BLOCKIO_PTR.store(raw, Ordering::Release);
                core::mem::forget(bio);
                storage_present = true;
                storage_blocks = inventory[inv_idx].blocks;
                storage_block_size = inventory[inv_idx].block_size;
                let total_mib = total / (1024 * 1024);
                storage_label = format!("UEFI BlockIO ({} MiB)", total_mib);
                STORAGE_BLOCK_SIZE.store(storage_block_size, Ordering::Release);
                STORAGE_TOTAL_BLOCKS.store(storage_blocks, Ordering::Release);
                inventory[inv_idx].picked_for_storage = true;
            }
        }
    }
    storage_inventory::record(inventory);

    UefiSubstrate {
        fb_info,
        storage_present,
        storage_blocks,
        storage_block_size,
        storage_label,
        keyboard_present,
    }
}

/// Identify the device handle our .efi was loaded from. Used to skip
/// the boot disk when picking persistent storage.
fn current_image_device_handle() -> Option<uefi::Handle> {
    let image_handle = uefi::boot::image_handle();
    let li: ScopedProtocol<LoadedImage> =
        uefi::boot::open_protocol_exclusive::<LoadedImage>(image_handle).ok()?;
    li.device()
}

/// Concatenated raw bytes of a device path's nodes (excluding the
/// terminator). Used for prefix comparison.
fn device_path_bytes(dp: &DevicePath) -> Vec<u8> {
    let mut out = Vec::new();
    for node in dp.node_iter() {
        // Each node has a 4-byte header (type, subtype, length-LE u16) plus its payload.
        let total = node.length() as usize;
        let raw = node.as_ffi_ptr() as *const u8;
        unsafe {
            let slice = core::slice::from_raw_parts(raw, total);
            out.extend_from_slice(slice);
        }
    }
    out
}

/// `prefix` is a strict (non-equal) prefix of `full`.
fn is_strict_prefix(prefix: &[u8], full: &[u8]) -> bool {
    prefix.len() < full.len() && full.starts_with(prefix)
}

fn device_label(blocks: u64, block_size: u64, is_partition: bool) -> String {
    let total = blocks.saturating_mul(block_size);
    let mib = total / (1024 * 1024);
    if is_partition {
        format!("partition ({} MiB)", mib)
    } else {
        format!("disk ({} MiB)", mib)
    }
}

fn pick_largest_mode(gop: &mut ScopedProtocol<GraphicsOutput>) -> Option<()> {
    // Cap framebuffer size so we don't allocate hundreds of MB on AAVMF's
    // huge default modes. Prefer Rgb/Bgr modes (linear FB) over BltOnly.
    const MAX_W: usize = 1280;
    const MAX_H: usize = 800;
    let mut best: Option<(usize, (usize, usize), bool)> = None;
    for (idx, mode) in gop.modes().enumerate() {
        let info = mode.info();
        let res = info.resolution();
        if res.0 > MAX_W || res.1 > MAX_H {
            continue;
        }
        let area = res.0 * res.1;
        let is_linear = matches!(
            info.pixel_format(),
            GopPixelFormat::Rgb | GopPixelFormat::Bgr
        );
        let better = match best {
            None => true,
            Some((_, (w, h), prev_linear)) => {
                if is_linear && !prev_linear {
                    true
                } else if !is_linear && prev_linear {
                    false
                } else {
                    area > w * h
                }
            }
        };
        if better {
            best = Some((idx, res, is_linear));
        }
    }
    let (idx, _, _) = best?;
    let target = gop.modes().nth(idx)?;
    gop.set_mode(&target).ok()?;
    Some(())
}

// ---------- shim op handlers ----------

/// Shared shim handlers for the UEFI ops that are identical across
/// arches. Per-arch kernels call these from their own Shim::execute.
pub fn handle_read_block(lba: u32, into: &mut [u8]) -> OpResult {
    let ptr = BLOCKIO_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        return OpResult::Block(BlockResult::NoDevice);
    }
    unsafe {
        let bio = &mut *ptr;
        let media_id = bio.media().media_id();
        let block_size = STORAGE_BLOCK_SIZE.load(Ordering::Acquire) as usize;
        let mut block_buf: Vec<u8> = vec![0u8; block_size];
        match bio.read_blocks(media_id, lba as u64, &mut block_buf) {
            Ok(()) => {
                let n = into.len().min(block_size);
                into[..n].copy_from_slice(&block_buf[..n]);
                OpResult::Block(BlockResult::Ok)
            }
            Err(_) => OpResult::Block(BlockResult::DeviceError(0)),
        }
    }
}

pub fn handle_write_block(lba: u32, from: &[u8]) -> OpResult {
    let ptr = BLOCKIO_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        return OpResult::Block(BlockResult::NoDevice);
    }
    unsafe {
        let bio = &mut *ptr;
        let media_id = bio.media().media_id();
        let block_size = STORAGE_BLOCK_SIZE.load(Ordering::Acquire) as usize;
        let mut block_buf: Vec<u8> = vec![0u8; block_size];
        let n = from.len().min(block_size);
        block_buf[..n].copy_from_slice(&from[..n]);
        match bio.write_blocks(media_id, lba as u64, &block_buf) {
            Ok(()) => OpResult::Block(BlockResult::Ok),
            Err(_) => OpResult::Block(BlockResult::DeviceError(0)),
        }
    }
}

pub fn handle_flush_storage() -> OpResult {
    let ptr = BLOCKIO_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        return OpResult::Done;
    }
    unsafe {
        let bio = &mut *ptr;
        let _ = bio.flush_blocks();
    }
    OpResult::Done
}

pub fn handle_poll_input() -> OpResult {
    OpResult::Input(poll_uefi_input())
}

pub fn handle_get_time() -> OpResult {
    let t = MONOTONIC_TICKS.fetch_add(1, Ordering::Relaxed);
    OpResult::Time(t)
}

pub fn handle_hash(bytes: &[u8]) -> OpResult {
    let h = kernel_core::blake3_hash(bytes);
    OpResult::Hash(*h.as_bytes())
}

fn poll_uefi_input() -> Option<u8> {
    let ptr = INPUT_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let input = &mut *ptr;
        match input.read_key() {
            Ok(Some(key)) => match key {
                uefi::proto::console::text::Key::Printable(c) => {
                    let ch: char = c.into();
                    if ch == '\r' || ch == '\n' {
                        Some(b'\n')
                    } else if (ch as u32) < 128 {
                        Some(ch as u8)
                    } else {
                        None
                    }
                }
                uefi::proto::console::text::Key::Special(_scancode) => None,
            },
            _ => None,
        }
    }
}

/// Push the kernel-core's framebuffer buffer to the screen via GOP Blt.
/// Each kernel calls this from its `Shim::present_frame`.
pub fn present_frame() {
    let gop_ptr = GOP_PTR.load(Ordering::Acquire);
    let buffer_ptr = FB_BUFFER_PTR.load(Ordering::Acquire);
    if gop_ptr.is_null() || buffer_ptr.is_null() {
        return;
    }
    let stride = FB_STRIDE.load(Ordering::Acquire) as usize;
    let width = FB_WIDTH.load(Ordering::Acquire) as usize;
    let height = FB_HEIGHT.load(Ordering::Acquire) as usize;
    unsafe {
        let gop = &mut *gop_ptr;
        let buffer = core::slice::from_raw_parts(
            buffer_ptr as *const BltPixel,
            stride * height,
        );
        let _ = gop.blt(BltOp::BufferToVideo {
            buffer,
            src: BltRegion::SubRectangle {
                coords: (0, 0),
                px_stride: stride,
            },
            dest: (0, 0),
            dims: (width, height),
        });
    }
}

// ---------- substrate observation ----------

pub fn observe_memory() -> Vec<MemoryRegion> {
    let map = match uefi::boot::memory_map(MemoryType::LOADER_DATA) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for desc in map.entries() {
        let size = desc.page_count * 4096;
        if size == 0 {
            continue;
        }
        let start = desc.phys_start;
        let end = start + size;
        let kind = match desc.ty {
            MemoryType::CONVENTIONAL => MemoryKind::Usable,
            MemoryType::LOADER_CODE | MemoryType::LOADER_DATA => MemoryKind::Bootloader,
            MemoryType::BOOT_SERVICES_CODE | MemoryType::BOOT_SERVICES_DATA => MemoryKind::Bootloader,
            MemoryType::ACPI_RECLAIM | MemoryType::ACPI_NON_VOLATILE => MemoryKind::Acpi,
            MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE => MemoryKind::Mmio,
            MemoryType::RESERVED | MemoryType::UNUSABLE => MemoryKind::Reserved,
            _ => MemoryKind::Other,
        };
        out.push(MemoryRegion { start, end, kind });
    }
    out
}

pub fn find_rsdp() -> Option<u64> {
    let mut found: Option<u64> = None;
    uefi::system::with_config_table(|entries| {
        for entry in entries {
            if entry.guid == ConfigTableEntry::ACPI2_GUID
                || entry.guid == ConfigTableEntry::ACPI_GUID
            {
                found = Some(entry.address as u64);
                return;
            }
        }
    });
    found
}
