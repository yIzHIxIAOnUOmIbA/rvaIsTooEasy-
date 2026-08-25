//! The `.rvapatch` self-contained patch container (Task B patch-ecosystem moat).
//!
//! Format: magic `RVPT` + version u16 + TLV sequence.
//! - TLV 0x01 metadata: source SHA256(32) + target SHA256(32) + timestamp i64 + engine version u32
//!   + strategy u8 + entry count u32 (81 bytes total).
//! - TLV 0x02 patch body: the Patch's Custom JSON serialization bytes.
//! - TLV 0x03 signature area (0..N records, appendable for multi-signature): fingerprint SHA256(public key)[0..32] + Ed25519 signature (64).
//! - TLV 0x04 rollback snapshot area (0..N segments, optional): old-byte snapshots of Modified/Removed
//!   so the patch can reverse back to the source file. Each segment = old_start u64 + len u32 + bytes.
//!
//! Signature target = magic + version + TLV 01 + TLV 02 + TLV 04 (naturally excluding the signature
//! area), matching the spec "SHA256 over all bytes except the signature area -> Ed25519 signature".
//! Appending a new signature never invalidates older ones.

use crate::patch_engine::{DefaultPatchEngine, Patch, PatchEngine, PatchOp};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub const PACK_MAGIC: &[u8; 4] = b"RVPT";
pub const PACK_VERSION: u16 = 1;
const TAG_META: u8 = 0x01;
const TAG_PATCH: u8 = 0x02;
const TAG_SIG: u8 = 0x03;
const TAG_REVERT: u8 = 0x04;

/// Rollback snapshot segment: old bytes in the source file that were replaced or removed.
#[derive(Debug, Clone)]
pub struct RevertSegment {
    pub old_start: u64,
    pub len: u32,
    pub bytes: Vec<u8>,
}

impl RevertSegment {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(12 + self.bytes.len());
        b.extend_from_slice(&self.old_start.to_le_bytes());
        b.extend_from_slice(&self.len.to_le_bytes());
        b.extend_from_slice(&self.bytes);
        b
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            bail!("回滚快照段过短: {} (至少 12)", data.len());
        }
        let old_start = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let len = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
        if 12 + len > data.len() {
            bail!("回滚快照段字节越界");
        }
        Ok(Self { old_start, len: len as u32, bytes: data[12..12 + len].to_vec() })
    }
}

/// Container metadata.
#[derive(Debug, Clone)]
pub struct PatchMeta {
    pub source_sha256: [u8; 32],
    pub target_sha256: [u8; 32],
    /// Unix timestamp in seconds (generation time).
    pub timestamp: i64,
    pub engine_version: u32,
    /// DiffStrategy id (0=ChunkedHash / 1=SlidingWindow / 2=Structural).
    pub strategy: u8,
    pub entry_count: u32,
}

impl PatchMeta {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(81);
        b.extend_from_slice(&self.source_sha256);
        b.extend_from_slice(&self.target_sha256);
        b.extend_from_slice(&self.timestamp.to_le_bytes());
        b.extend_from_slice(&self.engine_version.to_le_bytes());
        b.push(self.strategy);
        b.extend_from_slice(&self.entry_count.to_le_bytes());
        b
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() != 81 {
            bail!("元数据区长度非法: {} (期望 81)", data.len());
        }
        let mut src = [0u8; 32];
        let mut tgt = [0u8; 32];
        src.copy_from_slice(&data[0..32]);
        tgt.copy_from_slice(&data[32..64]);
        Ok(Self {
            source_sha256: src,
            target_sha256: tgt,
            timestamp: i64::from_le_bytes(data[64..72].try_into().unwrap()),
            engine_version: u32::from_le_bytes(data[72..76].try_into().unwrap()),
            strategy: data[76],
            entry_count: u32::from_le_bytes(data[77..81].try_into().unwrap()),
        })
    }
}

/// One signature record: fingerprint (first 32 bytes of SHA256 of the public key) + Ed25519 signature.
#[derive(Debug, Clone)]
pub struct PatchSignature {
    pub fingerprint: [u8; 32],
    pub signature: [u8; 64],
}

impl PatchSignature {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(96);
        b.extend_from_slice(&self.fingerprint);
        b.extend_from_slice(&self.signature);
        b
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() != 96 {
            bail!("签名记录长度非法: {} (期望 96)", data.len());
        }
        let mut fp = [0u8; 32];
        let mut sig = [0u8; 64];
        fp.copy_from_slice(&data[0..32]);
        sig.copy_from_slice(&data[32..96]);
        Ok(Self { fingerprint: fp, signature: sig })
    }
}

/// Complete `.rvapatch` container (in-memory representation).
#[derive(Debug, Clone)]
pub struct PackedPatch {
    pub version: u16,
    pub metadata: PatchMeta,
    /// The serialized Patch (Custom JSON).
    pub patch_bytes: Vec<u8>,
    /// Rollback snapshot segments (old bytes of Modified/Removed), letting rollback reverse to the source file.
    pub revert_segments: Vec<RevertSegment>,
    pub signatures: Vec<PatchSignature>,
}

impl PackedPatch {
    pub fn new(metadata: PatchMeta, patch: &Patch, revert_segments: Vec<RevertSegment>) -> Result<Self> {
        let patch_bytes = <DefaultPatchEngine as PatchEngine>::serialize(patch)?;
        Ok(Self {
            version: PACK_VERSION,
            metadata,
            patch_bytes,
            revert_segments,
            signatures: Vec::new(),
        })
    }

    pub fn to_patch(&self) -> Result<Patch> {
        <DefaultPatchEngine as PatchEngine>::deserialize(&self.patch_bytes)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// The bytes to sign (excluding any signature area); both signing and verification target this content.
    pub fn content_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(PACK_MAGIC);
        b.extend_from_slice(&self.version.to_le_bytes());
        let meta = self.metadata.encode();
        push_tlv(&mut b, TAG_META, &meta);
        push_tlv(&mut b, TAG_PATCH, &self.patch_bytes);
        if !self.revert_segments.is_empty() {
            let mut seg = Vec::new();
            for s in &self.revert_segments {
                seg.extend_from_slice(&s.encode());
            }
            push_tlv(&mut b, TAG_REVERT, &seg);
        }
        b
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = self.content_bytes();
        for s in &self.signatures {
            push_tlv(&mut b, TAG_SIG, &s.encode());
        }
        b
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 6 || &bytes[0..4] != PACK_MAGIC {
            bail!("不是有效的 .rvapatch 补丁容器（魔数不匹配），文件可能已被篡改或损坏");
        }
        let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        if version != PACK_VERSION {
            bail!("补丁容器版本校验失败（期望 {PACK_VERSION}，实际 {version}），文件可能已被篡改或损坏");
        }
        let mut cur = 6usize;
        let mut metadata: Option<PatchMeta> = None;
        let mut patch_bytes: Option<Vec<u8>> = None;
        let mut revert_segments: Vec<RevertSegment> = Vec::new();
        let mut signatures: Vec<PatchSignature> = Vec::new();
        while cur < bytes.len() {
            let (tag, data, next) = read_tlv(bytes, cur)?;
            match tag {
                TAG_META => metadata = Some(PatchMeta::decode(data)?),
                TAG_PATCH => patch_bytes = Some(data.to_vec()),
                TAG_REVERT => {
                    let mut off = 0usize;
                    while off < data.len() {
                        let seg = RevertSegment::decode(&data[off..])?;
                        off += 12 + seg.bytes.len();
                        revert_segments.push(seg);
                    }
                }
                TAG_SIG => signatures.push(PatchSignature::decode(data)?),
                t => bail!("未知 TLV 标签: 0x{t:02x}"),
            }
            cur = next;
        }
        let metadata = metadata.context("缺少元数据区")?;
        let patch_bytes = patch_bytes.context("缺少补丁体")?;
        Ok(Self { version, metadata, patch_bytes, revert_segments, signatures })
    }

    pub fn add_signature(&mut self, sig: PatchSignature) {
        self.signatures.push(sig);
    }
}

fn push_tlv(out: &mut Vec<u8>, tag: u8, data: &[u8]) {
    out.push(tag);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

fn read_tlv(bytes: &[u8], at: usize) -> Result<(u8, &[u8], usize)> {
    if at + 5 > bytes.len() {
        bail!("TLV 头部越界 (at {at})");
    }
    let tag = bytes[at];
    let len = u32::from_le_bytes(bytes[at + 1..at + 5].try_into().unwrap()) as usize;
    let end = at + 5 + len;
    if end > bytes.len() {
        bail!("TLV 数据越界 (tag 0x{tag:02x}, len {len})");
    }
    Ok((tag, &bytes[at + 5..end], end))
}

pub fn sha256_of(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Derive rollback snapshot segments from the patch ops and the source file bytes:
/// the gaps between Copy ranges are the replaced/deleted old bytes (Modified original bytes /
/// Removed content), and trailing leftovers likewise. Rollback fills these bytes back to restore
/// the source file.
pub fn build_revert_segments(old: &[u8], patch: &Patch) -> Vec<RevertSegment> {
    let mut segs: Vec<RevertSegment> = Vec::new();
    let mut old_pos: u64 = 0;
    for op in &patch.ops {
        if let PatchOp::Copy { from, len } = op {
            if *from > old_pos {
                let s = old_pos as usize;
                let e = (*from).min(old.len() as u64) as usize;
                if e > s {
                    segs.push(RevertSegment {
                        old_start: old_pos,
                        len: (e - s) as u32,
                        bytes: old[s..e].to_vec(),
                    });
                }
            }
            old_pos = *from + *len;
        }
    }
    if (old_pos as usize) < old.len() {
        segs.push(RevertSegment {
            old_start: old_pos,
            len: (old.len() - old_pos as usize) as u32,
            bytes: old[old_pos as usize..].to_vec(),
        });
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_engine::PatchOp;

    fn sample_meta() -> PatchMeta {
        PatchMeta {
            source_sha256: [1u8; 32],
            target_sha256: [2u8; 32],
            timestamp: 1_752_000_000,
            engine_version: 1,
            strategy: 1,
            entry_count: 2,
        }
    }

    fn sample_patch() -> Patch {
        Patch {
            format: crate::patch_engine::PatchFormat::Custom,
            old_size: 64,
            new_size: 72,
            ops: vec![
                PatchOp::Copy { from: 0, len: 16 },
                PatchOp::Insert { data: vec![0xAB; 8] },
                PatchOp::Copy { from: 24, len: 32 },
            ],
        }
    }

    fn sample_container() -> PackedPatch {
        let revert = build_revert_segments(&vec![0u8; 64], &sample_patch());
        PackedPatch::new(sample_meta(), &sample_patch(), revert).unwrap()
    }

    /// The magic and version constants anchor the format spec; any change is a breaking change and
    /// must bump the version first.
    #[test]
    fn format_anchors_stable() {
        assert_eq!(PACK_MAGIC, b"RVPT");
        assert_eq!(PACK_VERSION, 1);
        assert_eq!(TAG_META, 0x01);
        assert_eq!(TAG_PATCH, 0x02);
        assert_eq!(TAG_SIG, 0x03);
        assert_eq!(TAG_REVERT, 0x04);
    }

    /// The metadata area is fixed at 81 bytes; field offsets match docs/PATCH_FORMAT.md §2.1.
    #[test]
    fn metadata_layout_fixed() {
        let enc = sample_meta().encode();
        assert_eq!(enc.len(), 81);
        assert_eq!(&enc[0..32], &[1u8; 32]);
        assert_eq!(&enc[32..64], &[2u8; 32]);
        assert_eq!(i64::from_le_bytes(enc[64..72].try_into().unwrap()), 1_752_000_000);
        assert_eq!(u32::from_le_bytes(enc[72..76].try_into().unwrap()), 1);
        assert_eq!(enc[76], 1);
        assert_eq!(u32::from_le_bytes(enc[77..81].try_into().unwrap()), 2);
    }

    /// Full-container encode -> decode roundtrip is lossless.
    #[test]
    fn roundtrip_preserves_all() {
        let c = sample_container();
        let c2 = PackedPatch::from_bytes(&c.to_bytes()).unwrap();
        assert_eq!(c2.version, c.version);
        assert_eq!(c2.metadata.source_sha256, c.metadata.source_sha256);
        assert_eq!(c2.metadata.target_sha256, c.metadata.target_sha256);
        assert_eq!(c2.metadata.entry_count, c.metadata.entry_count);
        assert_eq!(c2.patch_bytes, c.patch_bytes);
        assert_eq!(c2.revert_segments.len(), c.revert_segments.len());
        assert_eq!(c2.to_patch().unwrap().ops.len(), c.to_patch().unwrap().ops.len());
    }

    /// Signature target = all non-signature-area bytes; appending a signature does not change content_bytes.
    #[test]
    fn content_bytes_excludes_signatures() {
        let mut c = sample_container();
        let before = c.content_bytes();
        c.add_signature(PatchSignature { fingerprint: [3u8; 32], signature: [4u8; 64] });
        assert_eq!(c.content_bytes(), before, "追加签名不得改变签名目标");
        let bytes = c.to_bytes();
        let c2 = PackedPatch::from_bytes(&bytes).unwrap();
        assert_eq!(c2.signatures.len(), 1);
    }

    /// Corrupted magic / version / structure -> rejected explicitly, with the error pointing at tampering/corruption.
    #[test]
    fn corrupted_container_rejected() {
        let mut bytes = sample_container().to_bytes();
        bytes[0] ^= 0xFF; // 破坏魔数
        assert!(PackedPatch::from_bytes(&bytes).is_err());
        let mut bytes = sample_container().to_bytes();
        bytes[5] = 0x02; // 版本 1 → 2
        assert!(PackedPatch::from_bytes(&bytes).is_err());
        let mut bytes = sample_container().to_bytes();
        bytes[6] = 0xFF; // 未知 TLV 标签
        assert!(PackedPatch::from_bytes(&bytes).is_err());
        let bytes = &sample_container().to_bytes()[..8]; // 截断
        assert!(PackedPatch::from_bytes(bytes).is_err());
    }

    /// Rollback snapshot segment roundtrip: bytes are unchanged after encode/decode.
    #[test]
    fn revert_segment_roundtrip() {
        let seg = RevertSegment { old_start: 10, len: 5, bytes: vec![7u8; 5] };
        let dec = RevertSegment::decode(&seg.encode()).unwrap();
        assert_eq!(dec.old_start, 10);
        assert_eq!(dec.bytes, vec![7u8; 5]);
        assert!(RevertSegment::decode(&[0u8; 4]).is_err());
    }
}
