//! Substrate-agnostic fabric snapshot (encode/decode + persist/restore
//! routed through the [`Shim`]'s ReadBlock/WriteBlock ops).
//!
//! Layout:
//!  - LBA 0      : superblock (512 bytes)
//!     magic(8) version(4) lamport(8) nodes(4) edges(4) body_len(8)
//!     blake3_checksum(32)
//!  - LBA 1..N+1 : snapshot body (length given by body_len)
//!
//! The fabric's "OS as data" claim is exactly this: the kernel's runtime
//! state IS this byte stream. The same encoded snapshot decodes on
//! either substrate without recompilation, because every NodeKind /
//! EdgeKind / agent matrix is a pure value, not a code pointer.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::fabric::{Edge, EdgeKind, Fabric, Node, NodeId, NodeKind};
use crate::ops::{BLOCK_SIZE, BlockResult, Op, OpResult, Shim};

const SUPERBLOCK_LBA: u32 = 0;
const SNAPSHOT_LBA: u32 = 1;
const MAGIC: u64 = 0xEC0_FAB_C09_DEEDA;
const VERSION: u32 = 1;

#[derive(Debug)]
pub struct Snapshot {
    pub lamport: u64,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug)]
pub enum SnapshotError {
    Io(String),
    BadMagic,
    BadVersion,
    BadChecksum,
    Truncated,
    Unparseable,
}

impl core::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SnapshotError::Io(s) => write!(f, "io: {}", s),
            SnapshotError::BadMagic => write!(f, "bad magic"),
            SnapshotError::BadVersion => write!(f, "bad version"),
            SnapshotError::BadChecksum => write!(f, "checksum mismatch"),
            SnapshotError::Truncated => write!(f, "truncated"),
            SnapshotError::Unparseable => write!(f, "unparseable"),
        }
    }
}

/// Apply a restored snapshot into the live fabric.
pub fn apply(fabric: &mut Fabric, snap: Snapshot) {
    fabric.nodes = snap.nodes;
    fabric.edges = snap.edges;
    fabric.lamport = snap.lamport;
}

/// Persist the entire fabric to the substrate's storage. Returns the
/// number of bytes written for the snapshot body (excluding superblock).
///
/// Uses the atomic-commit pattern: body first, flush, then the
/// superblock that names it (with the body's BLAKE3 checksum), then
/// flush again. This way the superblock is only valid after its body
/// is durable on disk — a crash or stop between superblock-write and
/// body-write can never produce a "fresh checksum, stale body"
/// situation. The pre-write flush is the difference between AAVMF in
/// QEMU (forgiving cache semantics on shutdown) and Parallels Desktop
/// (which holds writes in a host-side cache and won't flush them on
/// `prlctl stop --kill` or `--acpi` unless the guest issued an
/// explicit BlockIO FlushBlocks).
///
/// Multi-block bodies (the post-nucleation full-model snapshot is
/// ~150 KB → ~300 blocks at 512 B) need flushes *during* the body
/// write, not just at the end. Mac-CC's Session 25c retest reproduced
/// the cycle-1 flush regression on Apple Silicon Parallels even with
/// the trailing FlushStorage in place: the host-side write cache
/// defers a long sequential write run far enough that a `--kill`
/// landing immediately after `persist` can lose the still-in-flight
/// tail of the body. The fix is one flush per `FLUSH_EVERY_BLOCKS`
/// blocks during the body write, plus a read-back of the superblock
/// after the final flush to force any cache-coalesced pending writes
/// through the read path.
pub fn persist<S: Shim + ?Sized>(
    fabric: &Fabric,
    shim: &mut S,
) -> Result<usize, SnapshotError> {
    /// Flush cadence during body write. Tuned for the Apple Silicon
    /// Parallels host-cache behaviour observed in Session 25c — every
    /// 32 blocks ≈ every 16 KB on a 512 B substrate, giving Parallels
    /// regular sync points without an absurd number of FlushBlocks
    /// calls. A 150 KB body produces ~10 intermediate flushes.
    const FLUSH_EVERY_BLOCKS: usize = 32;

    let body = encode_snapshot(fabric);
    let body_len = body.len();
    let checksum = blake3::hash(&body);

    // 1. Write body blocks with a flush every FLUSH_EVERY_BLOCKS so
    //    Parallels' host cache can't defer the whole body until the
    //    very end.
    let mut lba = SNAPSHOT_LBA;
    let mut block_buf = [0u8; BLOCK_SIZE];
    for (i, chunk) in body.chunks(BLOCK_SIZE).enumerate() {
        block_buf.fill(0);
        block_buf[..chunk.len()].copy_from_slice(chunk);
        write_block(shim, lba, &block_buf)?;
        lba += 1;
        if (i + 1) % FLUSH_EVERY_BLOCKS == 0 {
            let _ = shim.execute(Op::FlushStorage);
        }
    }

    // 2. Flush body to durable storage BEFORE we name it from the superblock.
    let _ = shim.execute(Op::FlushStorage);

    // 3. Build + write the superblock, which contains the body's checksum.
    //    Now even a torn write of the superblock leaves either old-or-new
    //    valid pointers — never a new pointer at stale body bytes.
    let mut superblock = [0u8; BLOCK_SIZE];
    let mut sb = Vec::with_capacity(BLOCK_SIZE);
    put_u64(&mut sb, MAGIC);
    put_u32(&mut sb, VERSION);
    put_u64(&mut sb, fabric.lamport);
    put_u32(&mut sb, fabric.nodes.len() as u32);
    put_u32(&mut sb, fabric.edges.len() as u32);
    put_u64(&mut sb, body_len as u64);
    sb.extend_from_slice(checksum.as_bytes());
    superblock[..sb.len()].copy_from_slice(&sb);
    write_block(shim, SUPERBLOCK_LBA, &superblock)?;

    // 4. Flush superblock so the commit pointer survives the next stop.
    let _ = shim.execute(Op::FlushStorage);

    // 5. Read the superblock back. Read paths force any cache-coalesced
    //    pending writes through, because the cache hierarchy must
    //    return the most-recent value for the LBA. This is the
    //    quiescence sentinel: when this read returns, every prior
    //    write to LBA 0 is at least cache-consistent. On Apple Silicon
    //    Parallels this is the difference between cycle-1 surviving
    //    `--kill` and not.
    let mut readback = [0u8; BLOCK_SIZE];
    let _ = read_block(shim, SUPERBLOCK_LBA, &mut readback);

    Ok(body_len)
}

/// Read the snapshot back from the substrate's storage.
pub fn restore<S: Shim + ?Sized>(shim: &mut S) -> Result<Snapshot, SnapshotError> {
    let mut sb_buf = [0u8; BLOCK_SIZE];
    read_block(shim, SUPERBLOCK_LBA, &mut sb_buf)?;
    let mut off = 0;
    let magic = read_u64(&sb_buf, &mut off)?;
    if magic != MAGIC {
        return Err(SnapshotError::BadMagic);
    }
    let version = read_u32(&sb_buf, &mut off)?;
    if version != VERSION {
        return Err(SnapshotError::BadVersion);
    }
    let _lamport = read_u64(&sb_buf, &mut off)?;
    let _nodes = read_u32(&sb_buf, &mut off)?;
    let _edges = read_u32(&sb_buf, &mut off)?;
    let body_len = read_u64(&sb_buf, &mut off)? as usize;
    let mut expected_checksum = [0u8; 32];
    expected_checksum.copy_from_slice(&sb_buf[off..off + 32]);

    let body_blocks = body_len.div_ceil(BLOCK_SIZE);
    let mut body = vec![0u8; body_blocks * BLOCK_SIZE];
    for i in 0..body_blocks {
        let mut buf = [0u8; BLOCK_SIZE];
        read_block(shim, SNAPSHOT_LBA + i as u32, &mut buf)?;
        body[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE].copy_from_slice(&buf);
    }
    let body = &body[..body_len];

    let actual = blake3::hash(body);
    if actual.as_bytes() != &expected_checksum {
        return Err(SnapshotError::BadChecksum);
    }
    decode_snapshot(body)
}

fn write_block<S: Shim + ?Sized>(
    shim: &mut S,
    lba: u32,
    buf: &[u8; BLOCK_SIZE],
) -> Result<(), SnapshotError> {
    match shim.execute(Op::WriteBlock { lba, from: buf }) {
        OpResult::Block(BlockResult::Ok) => Ok(()),
        OpResult::Block(e) => Err(SnapshotError::Io(format!("write {}: {:?}", lba, e))),
        _ => Err(SnapshotError::Io("write: wrong op result".to_string())),
    }
}

fn read_block<S: Shim + ?Sized>(
    shim: &mut S,
    lba: u32,
    buf: &mut [u8; BLOCK_SIZE],
) -> Result<(), SnapshotError> {
    match shim.execute(Op::ReadBlock { lba, into: buf }) {
        OpResult::Block(BlockResult::Ok) => Ok(()),
        OpResult::Block(e) => Err(SnapshotError::Io(format!("read {}: {:?}", lba, e))),
        _ => Err(SnapshotError::Io("read: wrong op result".to_string())),
    }
}

// --- byte-level encode / decode ---

fn put_u8(out: &mut Vec<u8>, v: u8) { out.push(v); }
fn put_u16(out: &mut Vec<u8>, v: u16) { out.extend_from_slice(&v.to_le_bytes()); }
fn put_u32(out: &mut Vec<u8>, v: u32) { out.extend_from_slice(&v.to_le_bytes()); }
fn put_u64(out: &mut Vec<u8>, v: u64) { out.extend_from_slice(&v.to_le_bytes()); }
fn put_f32(out: &mut Vec<u8>, v: f32) { out.extend_from_slice(&v.to_le_bytes()); }
fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    put_u16(out, b.len() as u16);
    out.extend_from_slice(b);
}
fn put_bytes32(out: &mut Vec<u8>, b: &[u8; 32]) {
    out.extend_from_slice(b);
}

fn read_u8(b: &[u8], off: &mut usize) -> Result<u8, SnapshotError> {
    if *off + 1 > b.len() { return Err(SnapshotError::Truncated); }
    let v = b[*off]; *off += 1; Ok(v)
}
fn read_u16(b: &[u8], off: &mut usize) -> Result<u16, SnapshotError> {
    if *off + 2 > b.len() { return Err(SnapshotError::Truncated); }
    let v = u16::from_le_bytes(b[*off..*off+2].try_into().unwrap()); *off += 2; Ok(v)
}
fn read_u32(b: &[u8], off: &mut usize) -> Result<u32, SnapshotError> {
    if *off + 4 > b.len() { return Err(SnapshotError::Truncated); }
    let v = u32::from_le_bytes(b[*off..*off+4].try_into().unwrap()); *off += 4; Ok(v)
}
fn read_u64(b: &[u8], off: &mut usize) -> Result<u64, SnapshotError> {
    if *off + 8 > b.len() { return Err(SnapshotError::Truncated); }
    let v = u64::from_le_bytes(b[*off..*off+8].try_into().unwrap()); *off += 8; Ok(v)
}
fn read_f32(b: &[u8], off: &mut usize) -> Result<f32, SnapshotError> {
    if *off + 4 > b.len() { return Err(SnapshotError::Truncated); }
    let v = f32::from_le_bytes(b[*off..*off+4].try_into().unwrap()); *off += 4; Ok(v)
}
fn read_str(b: &[u8], off: &mut usize) -> Result<String, SnapshotError> {
    let len = read_u16(b, off)? as usize;
    if *off + len > b.len() { return Err(SnapshotError::Truncated); }
    let s = core::str::from_utf8(&b[*off..*off+len]).map_err(|_| SnapshotError::Unparseable)?;
    *off += len; Ok(s.to_string())
}
fn read_bytes32(b: &[u8], off: &mut usize) -> Result<[u8; 32], SnapshotError> {
    if *off + 32 > b.len() { return Err(SnapshotError::Truncated); }
    let mut a = [0u8; 32];
    a.copy_from_slice(&b[*off..*off+32]);
    *off += 32; Ok(a)
}

fn encode_node(out: &mut Vec<u8>, n: &Node) {
    put_bytes32(out, &n.id.0);
    put_u64(out, n.created_at);
    put_f32(out, n.weight);
    put_u8(out, n.kind.tag());
    match &n.kind {
        NodeKind::Genesis { fabric_lamport, observed } => {
            put_u64(out, *fabric_lamport);
            put_u32(out, *observed);
        }
        NodeKind::HwCpu { vendor, brand } => {
            put_str(out, vendor);
            put_str(out, brand);
        }
        NodeKind::HwCpuFeature(name) => put_str(out, name),
        NodeKind::HwMemoryRegion { start, end, kind } => {
            put_u64(out, *start);
            put_u64(out, *end);
            put_str(out, kind);
        }
        NodeKind::HwPciDevice { bus, device, function, vendor_id, device_id, class, subclass, prog_if } => {
            put_u8(out, *bus);
            put_u8(out, *device);
            put_u8(out, *function);
            put_u16(out, *vendor_id);
            put_u16(out, *device_id);
            put_u8(out, *class);
            put_u8(out, *subclass);
            put_u8(out, *prog_if);
        }
        NodeKind::HwAcpiTable { signature, address, length } => {
            out.extend_from_slice(signature);
            put_u64(out, *address);
            put_u32(out, *length);
        }
        NodeKind::HwFramebuffer { width, height, bytes_per_pixel, format } => {
            put_u32(out, *width);
            put_u32(out, *height);
            put_u8(out, *bytes_per_pixel);
            put_str(out, format);
        }
        NodeKind::HwStorage { kind, sectors, sector_size } => {
            put_str(out, kind);
            put_u64(out, *sectors);
            put_u32(out, *sector_size);
        }
        NodeKind::OperatorIntent { text, lamport }
        | NodeKind::FabricResponse { text, lamport }
        | NodeKind::SystemEvent { text, lamport } => {
            put_str(out, text);
            put_u64(out, *lamport);
        }
        NodeKind::LearnedDriver { kind, observations, avg_surprise_x1000, params } => {
            put_str(out, kind);
            put_u64(out, *observations);
            put_u32(out, *avg_surprise_x1000);
            put_u32(out, params.len() as u32);
            out.extend_from_slice(params);
        }
    }
}

fn decode_node(b: &[u8], off: &mut usize) -> Result<Node, SnapshotError> {
    let id = NodeId(read_bytes32(b, off)?);
    let created_at = read_u64(b, off)?;
    let weight = read_f32(b, off)?;
    let tag = read_u8(b, off)?;
    let kind = match tag {
        0 => NodeKind::Genesis { fabric_lamport: read_u64(b, off)?, observed: read_u32(b, off)? },
        1 => NodeKind::HwCpu { vendor: read_str(b, off)?, brand: read_str(b, off)? },
        2 => NodeKind::HwCpuFeature(read_str(b, off)?),
        3 => NodeKind::HwMemoryRegion { start: read_u64(b, off)?, end: read_u64(b, off)?, kind: read_str(b, off)? },
        4 => NodeKind::HwPciDevice {
            bus: read_u8(b, off)?, device: read_u8(b, off)?, function: read_u8(b, off)?,
            vendor_id: read_u16(b, off)?, device_id: read_u16(b, off)?,
            class: read_u8(b, off)?, subclass: read_u8(b, off)?, prog_if: read_u8(b, off)?,
        },
        5 => {
            let mut sig = [0u8; 4];
            for slot in sig.iter_mut() { *slot = read_u8(b, off)?; }
            NodeKind::HwAcpiTable { signature: sig, address: read_u64(b, off)?, length: read_u32(b, off)? }
        }
        6 => NodeKind::HwFramebuffer {
            width: read_u32(b, off)?, height: read_u32(b, off)?,
            bytes_per_pixel: read_u8(b, off)?, format: read_str(b, off)?,
        },
        7 => NodeKind::HwStorage { kind: read_str(b, off)?, sectors: read_u64(b, off)?, sector_size: read_u32(b, off)? },
        8 => NodeKind::OperatorIntent { text: read_str(b, off)?, lamport: read_u64(b, off)? },
        9 => NodeKind::FabricResponse { text: read_str(b, off)?, lamport: read_u64(b, off)? },
        10 => NodeKind::SystemEvent { text: read_str(b, off)?, lamport: read_u64(b, off)? },
        11 => {
            let kind = read_str(b, off)?;
            let observations = read_u64(b, off)?;
            let avg = read_u32(b, off)?;
            let n = read_u32(b, off)? as usize;
            if *off + n > b.len() { return Err(SnapshotError::Truncated); }
            let params = b[*off..*off + n].to_vec();
            *off += n;
            NodeKind::LearnedDriver { kind, observations, avg_surprise_x1000: avg, params }
        }
        _ => return Err(SnapshotError::Unparseable),
    };
    Ok(Node { id, kind, created_at, weight })
}

fn edge_kind_tag(k: EdgeKind) -> u8 {
    match k { EdgeKind::Contains => 0, EdgeKind::OnBus => 1, EdgeKind::Describes => 2, EdgeKind::Causes => 3 }
}

fn edge_kind_from_tag(t: u8) -> Result<EdgeKind, SnapshotError> {
    Ok(match t {
        0 => EdgeKind::Contains, 1 => EdgeKind::OnBus,
        2 => EdgeKind::Describes, 3 => EdgeKind::Causes,
        _ => return Err(SnapshotError::Unparseable),
    })
}

fn encode_snapshot(fabric: &Fabric) -> Vec<u8> {
    let mut body = Vec::with_capacity(64 * 1024);
    put_u32(&mut body, fabric.nodes.len() as u32);
    put_u32(&mut body, fabric.edges.len() as u32);
    put_u64(&mut body, fabric.lamport);
    for n in &fabric.nodes { encode_node(&mut body, n); }
    for e in &fabric.edges {
        put_bytes32(&mut body, &e.source.0);
        put_bytes32(&mut body, &e.target.0);
        put_u8(&mut body, edge_kind_tag(e.kind));
    }
    body
}

fn decode_snapshot(body: &[u8]) -> Result<Snapshot, SnapshotError> {
    let mut off = 0;
    let n_nodes = read_u32(body, &mut off)?;
    let n_edges = read_u32(body, &mut off)?;
    let lamport = read_u64(body, &mut off)?;
    let mut nodes = Vec::with_capacity(n_nodes as usize);
    for _ in 0..n_nodes { nodes.push(decode_node(body, &mut off)?); }
    let mut edges = Vec::with_capacity(n_edges as usize);
    for _ in 0..n_edges {
        let source = NodeId(read_bytes32(body, &mut off)?);
        let target = NodeId(read_bytes32(body, &mut off)?);
        let kind = edge_kind_from_tag(read_u8(body, &mut off)?)?;
        edges.push(Edge { source, target, kind });
    }
    Ok(Snapshot { lamport, nodes, edges })
}
