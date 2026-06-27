//! Reader for the GROMACS XTC compressed-trajectory format.
//!
//! XTC stores a stream of frames, each a small header (atom count, step, time,
//! box) followed by lossily compressed coordinates. The compression is the
//! portable XDR-based 3D-coordinate scheme: coordinates are quantized to a
//! precision, delta-encoded against neighbours, and bit-packed. This module
//! implements the *decode* half of that scheme so trajectories can be played
//! back without shelling out to an external tool.
//!
//! Coordinates on disk are in nanometers; they are converted to Angstrom here so
//! frames line up with [`crate::domain::Structure`] (same factor the `.gro`
//! loader applies).

use std::fs::File;
use std::io::{BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::domain::Trajectory;

/// nm -> Angstrom, matching the `.gro` loader (`io/formats/gro.rs`).
const NM_TO_ANGSTROM: f32 = 10.0;

/// Frame magic numbers. 1995 is the classic format (32-bit packed-byte count);
/// 2023 is the large-system format (64-bit count). Coordinates decode
/// identically; only the count width differs.
const XTC_MAGIC: i32 = 1995;
const XTC_NEW_MAGIC: i32 = 2023;

/// First index into `MAGICINTS` whose value is non-zero.
const FIRSTIDX: i32 = 9;

/// Quantization-bucket table shared by encoder and decoder. `MAGICINTS[i]` is
/// the number of distinct values representable at small-index `i`. These values
/// are fixed by the XTC format; correctness depends on an exact match.
const MAGICINTS: [i32; 73] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 10, 12, 16, 20, 25, 32, 40, 50, 64, 80, 101, 128, 161, 203, 256,
    322, 406, 512, 645, 812, 1024, 1290, 1625, 2048, 2580, 3250, 4096, 5060, 6501, 8192, 10321,
    13003, 16384, 20642, 26007, 32768, 41285, 52015, 65536, 82570, 104031, 131072, 165140, 208063,
    262144, 330280, 416127, 524287, 660561, 832255, 1048576, 1321122, 1664510, 2097152, 2642245,
    3329021, 4194304, 5284491, 6658042, 8388607, 10568983, 13316085, 16777216,
];

const LASTIDX: i32 = MAGICINTS.len() as i32;

/// Upper bound on decoded coordinate floats held in memory (~256 MB). Larger
/// trajectories are subsampled with a frame stride so playback stays bounded;
/// the stride is recorded on the returned [`Trajectory`].
const MAX_COORD_FLOATS: usize = 64_000_000;

/// Read an XTC trajectory from `path`, converting coordinates to Angstrom.
///
/// The base structure's atom count is not required; it is read from the file.
/// If the file holds more frames than fit in the memory budget, frames are
/// subsampled with a uniform stride (recorded on the result).
pub fn read_xtc(path: &Path) -> Result<Trajectory> {
    let file =
        File::open(path).with_context(|| format!("failed to open XTC file {}", path.display()))?;
    read_xtc_from(BufReader::new(file))
        .with_context(|| format!("failed to read XTC file {}", path.display()))
}

/// Decode an XTC stream from any seekable reader (the file-backed [`read_xtc`]
/// is a thin wrapper; tests drive this directly with an in-memory cursor).
fn read_xtc_from<R: Read + Seek>(mut reader: R) -> Result<Trajectory> {
    // Pass 1: scan frame headers to learn the atom count, frame count, each
    // frame's start offset, and its time. Coordinate blocks are skipped, not
    // decoded.
    let mut frame_offsets: Vec<u64> = Vec::new();
    let mut frame_times: Vec<f32> = Vec::new();
    let mut natoms = 0usize;
    loop {
        let offset = reader.stream_position()?;
        let Some(magic) = read_optional_i32(&mut reader)? else {
            break;
        };
        check_magic(magic)?;
        let frame_natoms = read_i32(&mut reader)? as usize;
        if frame_natoms == 0 {
            bail!("XTC frame reports zero atoms");
        }
        if frame_offsets.is_empty() {
            natoms = frame_natoms;
        } else if frame_natoms != natoms {
            bail!(
                "XTC atom count changes between frames ({natoms} then {frame_natoms}); \
                 trajectory playback requires a constant atom count"
            );
        }
        let _step = read_i32(&mut reader)?;
        let time = read_f32(&mut reader)?;
        // Box: 3x3 floats.
        reader.seek(SeekFrom::Current(9 * 4))?;
        skip_coord_block(&mut reader, magic)?;
        frame_offsets.push(offset);
        frame_times.push(time);
    }

    let source_frame_count = frame_offsets.len();
    if source_frame_count == 0 || natoms == 0 {
        return Ok(Trajectory::from_parts(natoms, Vec::new(), Vec::new(), 1, 0));
    }

    // Subsample if the full trajectory would exceed the memory budget.
    let max_frames = (MAX_COORD_FLOATS / (natoms * 3)).max(1);
    let stride = source_frame_count.div_ceil(max_frames).max(1);

    let kept: Vec<usize> = (0..source_frame_count).step_by(stride).collect();
    let mut coords = Vec::with_capacity(kept.len() * natoms * 3);
    let mut times = Vec::with_capacity(kept.len());
    let mut scratch = vec![0f32; natoms * 3];

    // Pass 2: seek to each kept frame and decode its coordinates.
    for &frame in &kept {
        reader.seek(SeekFrom::Start(frame_offsets[frame]))?;
        decode_frame(&mut reader, natoms, &mut scratch)?;
        coords.extend_from_slice(&scratch);
        times.push(frame_times[frame]);
    }

    Ok(Trajectory::from_parts(
        natoms,
        coords,
        times,
        stride,
        source_frame_count,
    ))
}

fn check_magic(magic: i32) -> Result<()> {
    if magic != XTC_MAGIC && magic != XTC_NEW_MAGIC {
        bail!("not an XTC file (bad magic number {magic})");
    }
    Ok(())
}

/// Read a frame's full header (magic..box) then decode its coordinate block into
/// `out` (length `natoms * 3`, Angstrom). The reader must be positioned at the
/// frame's `magic`.
fn decode_frame<R: Read + Seek>(reader: &mut R, natoms: usize, out: &mut [f32]) -> Result<()> {
    let magic = read_i32(reader)?;
    check_magic(magic)?;
    let frame_natoms = read_i32(reader)? as usize;
    if frame_natoms != natoms {
        bail!("XTC frame atom count {frame_natoms} != expected {natoms}");
    }
    let _step = read_i32(reader)?;
    let _time = read_f32(reader)?;
    reader.seek(SeekFrom::Current(9 * 4))?; // box
    decode_coords(reader, natoms, magic, out)
}

/// Skip a frame's coordinate sub-record (used by the scanning pass).
fn skip_coord_block<R: Read + Seek>(reader: &mut R, magic: i32) -> Result<()> {
    let size = read_i32(reader)? as i64; // atom count, repeated
    if size <= 9 {
        // Uncompressed: 3 floats per atom.
        reader.seek(SeekFrom::Current(size * 3 * 4))?;
    } else {
        // precision (1 float) + minint[3] + maxint[3] + smallidx (1 int).
        reader.seek(SeekFrom::Current(4 + 6 * 4 + 4))?;
        let nbytes = if magic == XTC_NEW_MAGIC {
            read_i64(reader)?
        } else {
            read_i32(reader)? as i64
        };
        // XDR opaque rounds the byte count up to a 4-byte boundary.
        reader.seek(SeekFrom::Current(round_up_4(nbytes)))?;
    }
    Ok(())
}

/// Decode one coordinate sub-record into `out` (Angstrom). The reader must be
/// positioned at the record's repeated atom count.
// `prevcoord` holds the running previous coordinate used for delta decoding: it
// is assigned every iteration (and on the final one the value is unused), so the
// dead-store lint is expected here.
#[allow(unused_assignments)]
fn decode_coords<R: Read + Seek>(
    reader: &mut R,
    natoms: usize,
    magic: i32,
    out: &mut [f32],
) -> Result<()> {
    debug_assert_eq!(out.len(), natoms * 3);
    let size = read_i32(reader)? as usize;
    if size != natoms {
        bail!("XTC coordinate count {size} != expected {natoms}");
    }

    if size <= 9 {
        // Small systems store raw big-endian floats (nm).
        for slot in out.iter_mut() {
            *slot = read_f32(reader)? * NM_TO_ANGSTROM;
        }
        return Ok(());
    }

    let precision = read_f32(reader)?;
    let mut minint = [0i32; 3];
    let mut maxint = [0i32; 3];
    for value in &mut minint {
        *value = read_i32(reader)?;
    }
    for value in &mut maxint {
        *value = read_i32(reader)?;
    }

    let mut sizeint = [0u32; 3];
    for i in 0..3 {
        sizeint[i] = (maxint[i] as i64 - minint[i] as i64 + 1) as u32;
    }

    // When a span is too large to multiply together, each component is packed
    // with its own bit width; otherwise the three pack into one integer.
    let mut bitsizeint = [0i32; 3];
    let bitsize;
    if (sizeint[0] | sizeint[1] | sizeint[2]) > 0xffffff {
        bitsizeint[0] = sizeofint(sizeint[0]);
        bitsizeint[1] = sizeofint(sizeint[1]);
        bitsizeint[2] = sizeofint(sizeint[2]);
        bitsize = 0;
    } else {
        bitsize = sizeofints(&sizeint);
    }

    let mut smallidx = read_i32(reader)?;
    if !(FIRSTIDX..LASTIDX).contains(&smallidx) {
        bail!("XTC small index {smallidx} out of range");
    }

    let nbytes = if magic == XTC_NEW_MAGIC {
        read_i64(reader)? as usize
    } else {
        read_i32(reader)? as usize
    };

    // The bit reader may consume a few bytes past the encoded payload; pad with
    // zeros so it never runs off the end on a well-formed file.
    let mut data = vec![0u8; nbytes + 16];
    reader
        .read_exact(&mut data[..nbytes])
        .context("truncated XTC coordinate block")?;
    let pad = round_up_4(nbytes as i64) - nbytes as i64;
    if pad > 0 {
        reader.seek(SeekFrom::Current(pad))?;
    }

    let mut buffer = BitReader::new(&data);

    let inv_precision = 1.0 / precision;
    let factor = inv_precision * NM_TO_ANGSTROM;

    let mut smallnum = MAGICINTS[smallidx as usize] / 2;
    let mut smaller = MAGICINTS[FIRSTIDX.max(smallidx - 1) as usize] / 2;
    let mut sizesmall = [MAGICINTS[smallidx as usize] as u32; 3];

    let lsize = natoms as i32;
    let mut i = 0i32;
    let mut out_index = 0usize;
    let mut thiscoord = [0i32; 3];
    let mut prevcoord = [0i32; 3];
    // `run` persists across iterations: a `flag` of 0 means "run length
    // unchanged from the previous atom", so it must not be reset each loop.
    let mut run = 0i32;

    while i < lsize {
        if bitsize == 0 {
            thiscoord[0] = buffer.receivebits(bitsizeint[0]);
            thiscoord[1] = buffer.receivebits(bitsizeint[1]);
            thiscoord[2] = buffer.receivebits(bitsizeint[2]);
        } else {
            receiveints(&mut buffer, bitsize, &sizeint, &mut thiscoord);
        }
        i += 1;
        thiscoord[0] += minint[0];
        thiscoord[1] += minint[1];
        thiscoord[2] += minint[2];
        prevcoord = thiscoord;

        let flag = buffer.receivebits(1);
        let mut is_smaller = 0i32;
        if flag == 1 {
            run = buffer.receivebits(5);
            is_smaller = run % 3;
            run -= is_smaller;
            is_smaller -= 1;
        }

        if run > 0 {
            let mut k = 0i32;
            while k < run {
                receiveints(&mut buffer, smallidx, &sizesmall, &mut thiscoord);
                i += 1;
                thiscoord[0] += prevcoord[0] - smallnum;
                thiscoord[1] += prevcoord[1] - smallnum;
                thiscoord[2] += prevcoord[2] - smallnum;
                if k == 0 {
                    // Swap the first two atoms of a run (the encoder does this
                    // to compress water molecules better).
                    std::mem::swap(&mut thiscoord[0], &mut prevcoord[0]);
                    std::mem::swap(&mut thiscoord[1], &mut prevcoord[1]);
                    std::mem::swap(&mut thiscoord[2], &mut prevcoord[2]);
                    write_coord(out, &mut out_index, &prevcoord, factor)?;
                } else {
                    prevcoord = thiscoord;
                }
                write_coord(out, &mut out_index, &thiscoord, factor)?;
                k += 3;
            }
        } else {
            write_coord(out, &mut out_index, &thiscoord, factor)?;
        }

        smallidx += is_smaller;
        if !(0..LASTIDX).contains(&smallidx) {
            bail!("XTC small index drifted out of range (corrupt frame)");
        }
        if is_smaller < 0 {
            smallnum = smaller;
            smaller = if smallidx > FIRSTIDX {
                MAGICINTS[(smallidx - 1) as usize] / 2
            } else {
                0
            };
        } else if is_smaller > 0 {
            smaller = smallnum;
            smallnum = MAGICINTS[smallidx as usize] / 2;
        }
        sizesmall = [MAGICINTS[smallidx as usize] as u32; 3];
    }

    if out_index != out.len() {
        bail!(
            "XTC frame decoded {} of {} coordinates (corrupt frame)",
            out_index,
            out.len()
        );
    }
    Ok(())
}

fn write_coord(out: &mut [f32], index: &mut usize, coord: &[i32; 3], factor: f32) -> Result<()> {
    if *index + 3 > out.len() {
        bail!("XTC frame produced more coordinates than atoms (corrupt frame)");
    }
    out[*index] = coord[0] as f32 * factor;
    out[*index + 1] = coord[1] as f32 * factor;
    out[*index + 2] = coord[2] as f32 * factor;
    *index += 3;
    Ok(())
}

fn round_up_4(n: i64) -> i64 {
    (n + 3) & !3
}

/// Bits needed to store a non-negative integer up to `size`.
fn sizeofint(size: u32) -> i32 {
    let mut num: i64 = 1;
    let mut bits = 0i32;
    while size as i64 >= num && bits < 32 {
        bits += 1;
        num <<= 1;
    }
    bits
}

/// Bits needed to pack three integers with the given per-component sizes into a
/// single value (the inverse of the encoder's packing width).
fn sizeofints(sizes: &[u32; 3]) -> i32 {
    let mut bytes = [0u32; 32];
    let mut num_of_bytes = 1usize;
    bytes[0] = 1;
    for &size in sizes {
        let mut tmp: u64 = 0;
        let mut bytecnt = 0usize;
        while bytecnt < num_of_bytes {
            tmp += bytes[bytecnt] as u64 * size as u64;
            bytes[bytecnt] = (tmp & 0xff) as u32;
            tmp >>= 8;
            bytecnt += 1;
        }
        while tmp != 0 {
            bytes[bytecnt] = (tmp & 0xff) as u32;
            bytecnt += 1;
            tmp >>= 8;
        }
        num_of_bytes = bytecnt;
    }
    let mut num: u32 = 1;
    let mut num_of_bits = 0i32;
    num_of_bytes -= 1;
    while bytes[num_of_bytes] >= num {
        num_of_bits += 1;
        num *= 2;
    }
    num_of_bits + num_of_bytes as i32 * 8
}

/// Decode three small integers packed with `num_of_bits` total bits, splitting
/// them back out via division by `sizes` (inverse of the encoder's multiply).
fn receiveints(
    buffer: &mut BitReader<'_>,
    num_of_bits: i32,
    sizes: &[u32; 3],
    nums: &mut [i32; 3],
) {
    let mut bytes = [0i32; 32];
    let mut num_of_bytes = 0usize;
    let mut remaining = num_of_bits;
    while remaining > 8 {
        bytes[num_of_bytes] = buffer.receivebits(8);
        num_of_bytes += 1;
        remaining -= 8;
    }
    if remaining > 0 {
        bytes[num_of_bytes] = buffer.receivebits(remaining);
        num_of_bytes += 1;
    }
    for i in (1..3).rev() {
        let mut num = 0i32;
        for j in (0..num_of_bytes).rev() {
            num = (num << 8) | bytes[j];
            let p = num / sizes[i] as i32;
            bytes[j] = p;
            num -= p * sizes[i] as i32;
        }
        nums[i] = num;
    }
    nums[0] = bytes[0] | (bytes[1] << 8) | (bytes[2] << 16) | (bytes[3] << 24);
}

/// MSB-first bit reader over the packed coordinate buffer, tracking the XTC
/// bit-stream state (byte cursor + partial-byte accumulator).
struct BitReader<'a> {
    data: &'a [u8],
    index: usize,
    lastbits: i32,
    lastbyte: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            index: 0,
            lastbits: 0,
            lastbyte: 0,
        }
    }

    /// Reads past the buffer return zeros, so a corrupt/short payload degrades
    /// gracefully instead of panicking.
    fn next_byte(&mut self) -> u32 {
        let byte = self.data.get(self.index).copied().unwrap_or(0);
        self.index += 1;
        byte as u32
    }

    fn receivebits(&mut self, mut num_of_bits: i32) -> i32 {
        let mask: u32 = ((1u64 << num_of_bits) - 1) as u32;
        let mut lastbits = self.lastbits;
        let mut lastbyte = self.lastbyte;
        let mut num: u32 = 0;
        while num_of_bits >= 8 {
            lastbyte = (lastbyte << 8) | self.next_byte();
            num |= (lastbyte >> lastbits as u32) << (num_of_bits - 8) as u32;
            num_of_bits -= 8;
        }
        if num_of_bits > 0 {
            if lastbits < num_of_bits {
                lastbits += 8;
                lastbyte = (lastbyte << 8) | self.next_byte();
            }
            lastbits -= num_of_bits;
            num |= (lastbyte >> lastbits as u32) & ((1u32 << num_of_bits) - 1);
        }
        num &= mask;
        self.lastbits = lastbits;
        self.lastbyte = lastbyte;
        num as i32
    }
}

fn read_i32<R: Read>(reader: &mut R) -> Result<i32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(i32::from_be_bytes(bytes))
}

/// Read a 4-byte big-endian int, or `None` on a clean end-of-file (used to
/// detect the end of the frame stream).
fn read_optional_i32<R: Read>(reader: &mut R) -> Result<Option<i32>> {
    let mut bytes = [0u8; 4];
    match reader.read_exact(&mut bytes) {
        Ok(()) => Ok(Some(i32::from_be_bytes(bytes))),
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_i64<R: Read>(reader: &mut R) -> Result<i64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(i64::from_be_bytes(bytes))
}

fn read_f32<R: Read>(reader: &mut R) -> Result<f32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(f32::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests;
