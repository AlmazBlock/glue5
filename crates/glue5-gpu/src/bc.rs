//! BCn texture data decoded on the CPU, for devices without BC compression
//! (iPhones before the A17 Pro, most Android GPUs).
//!
//! Decoded formats: BC1/2/3/7 -> RGBA8, BC4 -> R8, BC5 -> RG8, BC6H -> RGBA16F.

use crate::format::Bc;

impl Bc {
    /// Bytes per compressed 4x4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            Bc::Bc1 | Bc::Bc4 => 8,
            _ => 16,
        }
    }

    /// Bytes per decoded texel.
    pub fn texel_bytes(self) -> usize {
        match self {
            Bc::Bc4 => 1,
            Bc::Bc5 => 2,
            Bc::Bc6h => 8,
            _ => 4,
        }
    }
}

/// Decodes `cols` x `rows` blocks per layer (`layers` layers of `src_rows`
/// block rows, `src_bpr` bytes per block row) into tightly packed texels:
/// `cols * 4` texels per row, `rows * 4` rows per layer.
pub fn decode(bc: Bc, src: &[u8], src_bpr: usize, src_rows: usize, cols: usize, rows: usize, layers: usize) -> Vec<u8> {
    let texel = bc.texel_bytes();
    let pitch = cols * 4 * texel;
    let band = pitch * 4; // one row of blocks
    let mut out = vec![0u8; band * rows * layers];
    // Block rows, all layers: split into bands across threads for big uploads.
    let jobs: Vec<(usize, &mut [u8])> = out.chunks_mut(band).enumerate().collect();
    let threads = if cols * rows * layers >= 4096 {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min(8)
    } else {
        1
    };
    let decode_band = |(i, dst): (usize, &mut [u8])| {
        let (layer, row) = (i / rows, i % rows);
        let src_row = (layer * src_rows + row) * src_bpr;
        for col in 0..cols {
            let at = src_row + col * bc.block_bytes();
            let Some(block) = src.get(at..at + bc.block_bytes()) else { return };
            decode_block(bc, block, &mut dst[col * 4 * texel..], pitch);
        }
    };
    if threads <= 1 {
        jobs.into_iter().for_each(decode_band);
    } else {
        let per = jobs.len().div_ceil(threads);
        let mut jobs = jobs;
        std::thread::scope(|s| {
            while !jobs.is_empty() {
                let rest = jobs.split_off(per.min(jobs.len()));
                let chunk = std::mem::replace(&mut jobs, rest);
                s.spawn(move || chunk.into_iter().for_each(decode_band));
            }
        });
    }
    out
}

/// One block into `dst` (its top-left texel), `pitch` bytes per texel row.
fn decode_block(bc: Bc, block: &[u8], dst: &mut [u8], pitch: usize) {
    match bc {
        Bc::Bc1 => bcdec_rs::bc1(block, dst, pitch),
        Bc::Bc2 => bcdec_rs::bc2(block, dst, pitch),
        Bc::Bc3 => bcdec_rs::bc3(block, dst, pitch),
        Bc::Bc4 => bcdec_rs::bc4(block, dst, pitch, false),
        Bc::Bc5 => bcdec_rs::bc5(block, dst, pitch, false),
        Bc::Bc7 => bcdec_rs::bc7(block, dst, pitch),
        Bc::Bc6h => {
            // RGB halves, then RGBA16F with alpha 1.0.
            let mut rgb = [0u16; 4 * 4 * 3];
            bcdec_rs::bc6h_half(block, &mut rgb, 4 * 3, false);
            for y in 0..4 {
                for x in 0..4 {
                    let s = (y * 4 + x) * 3;
                    let d = y * pitch + x * 8;
                    for (c, v) in [rgb[s], rgb[s + 1], rgb[s + 2], 0x3c00].into_iter().enumerate() {
                        dst[d + c * 2..d + c * 2 + 2].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
        }
    }
}
