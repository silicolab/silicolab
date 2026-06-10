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
mod tests {
    use std::io::Cursor;

    use super::*;

    /// Big-endian writers matching the XDR scalars the reader consumes.
    fn push_i32(out: &mut Vec<u8>, value: i32) {
        out.extend_from_slice(&value.to_be_bytes());
    }
    fn push_f32(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_be_bytes());
    }

    /// Bit-stream encoder implementing the XTC `sendbits`/`sendints` write
    /// path. Used only to produce fixtures the reader must decode back, so the
    /// round-trip exercises the full decompressor.
    struct Encoder {
        data: Vec<u8>,
        index: usize,
        lastbits: i32,
        lastbyte: u32,
    }

    impl Encoder {
        fn new(capacity: usize) -> Self {
            Self {
                data: vec![0u8; capacity],
                index: 0,
                lastbits: 0,
                lastbyte: 0,
            }
        }

        fn sendbits(&mut self, mut num_of_bits: i32, num: i32) {
            let mut lastbyte = self.lastbyte;
            let mut lastbits = self.lastbits;
            while num_of_bits >= 8 {
                lastbyte = (lastbyte << 8) | ((num >> (num_of_bits - 8)) as u32);
                self.data[self.index] = (lastbyte >> lastbits) as u8;
                self.index += 1;
                num_of_bits -= 8;
            }
            if num_of_bits > 0 {
                lastbyte = (lastbyte << num_of_bits) | (num as u32);
                lastbits += num_of_bits;
                if lastbits >= 8 {
                    lastbits -= 8;
                    self.data[self.index] = (lastbyte >> lastbits) as u8;
                    self.index += 1;
                }
            }
            self.lastbits = lastbits;
            self.lastbyte = lastbyte;
            if lastbits > 0 {
                self.data[self.index] = (lastbyte << (8 - lastbits)) as u8;
            }
        }

        fn sendints(&mut self, num_of_bits: i32, sizes: &[u32; 3], nums: &[u32; 3]) {
            let mut bytes = [0u32; 32];
            let mut tmp = nums[0] as u64;
            let mut num_of_bytes = 0usize;
            loop {
                bytes[num_of_bytes] = (tmp & 0xff) as u32;
                num_of_bytes += 1;
                tmp >>= 8;
                if tmp == 0 {
                    break;
                }
            }
            for i in 1..3 {
                let mut t = nums[i] as u64;
                let mut bytecnt = 0usize;
                while bytecnt < num_of_bytes {
                    t += bytes[bytecnt] as u64 * sizes[i] as u64;
                    bytes[bytecnt] = (t & 0xff) as u32;
                    t >>= 8;
                    bytecnt += 1;
                }
                while t != 0 {
                    bytes[bytecnt] = (t & 0xff) as u32;
                    bytecnt += 1;
                    t >>= 8;
                }
                num_of_bytes = bytecnt;
            }
            let nb = num_of_bytes as i32;
            if num_of_bits >= nb * 8 {
                for &byte in bytes.iter().take(num_of_bytes) {
                    self.sendbits(8, byte as i32);
                }
                self.sendbits(num_of_bits - nb * 8, 0);
            } else {
                for &byte in bytes.iter().take(num_of_bytes - 1) {
                    self.sendbits(8, byte as i32);
                }
                self.sendbits(num_of_bits - (nb - 1) * 8, bytes[num_of_bytes - 1] as i32);
            }
        }
    }

    /// Append a full XTC frame (header + compressed coordinates) for `coords_nm`
    /// (nm, length `natoms * 3`) to `out`.
    #[allow(clippy::too_many_arguments)]
    fn encode_frame(
        out: &mut Vec<u8>,
        magic: i32,
        natoms: usize,
        step: i32,
        time: f32,
        box9: &[f32; 9],
        coords_nm: &[f32],
        precision: f32,
    ) {
        push_i32(out, magic);
        push_i32(out, natoms as i32);
        push_i32(out, step);
        push_f32(out, time);
        for &value in box9 {
            push_f32(out, value);
        }

        push_i32(out, natoms as i32);
        if natoms <= 9 {
            for &value in coords_nm {
                push_f32(out, value);
            }
            return;
        }

        push_f32(out, precision);

        // Quantize to integers and track per-component extents + the smallest
        // step between consecutive atoms (drives the initial small index).
        let mut minint = [i32::MAX; 3];
        let mut maxint = [i32::MIN; 3];
        let mut ip = vec![0i32; natoms * 3];
        let mut mindiff = i32::MAX;
        let mut old = [0i32; 3];
        for atom in 0..natoms {
            let mut current = [0i32; 3];
            for c in 0..3 {
                let scaled = coords_nm[atom * 3 + c] * precision;
                let rounded = if scaled >= 0.0 {
                    scaled + 0.5
                } else {
                    scaled - 0.5
                };
                let value = rounded as i32;
                minint[c] = minint[c].min(value);
                maxint[c] = maxint[c].max(value);
                ip[atom * 3 + c] = value;
                current[c] = value;
            }
            if atom > 0 {
                let diff = (old[0] - current[0]).abs()
                    + (old[1] - current[1]).abs()
                    + (old[2] - current[2]).abs();
                mindiff = mindiff.min(diff);
            }
            old = current;
        }

        for value in minint {
            push_i32(out, value);
        }
        for value in maxint {
            push_i32(out, value);
        }

        let mut sizeint = [0u32; 3];
        for c in 0..3 {
            sizeint[c] = (maxint[c] as i64 - minint[c] as i64 + 1) as u32;
        }
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

        let mut smallidx = FIRSTIDX;
        while smallidx < LASTIDX && MAGICINTS[smallidx as usize] < mindiff {
            smallidx += 1;
        }
        assert!(
            smallidx < LASTIDX,
            "test coordinates too spread out to encode"
        );
        push_i32(out, smallidx);

        let maxidx = (smallidx + 8).min(LASTIDX - 1);
        let minidx = maxidx - 8;
        let mut smaller = MAGICINTS[FIRSTIDX.max(smallidx - 1) as usize] / 2;
        let mut smallnum = MAGICINTS[smallidx as usize] / 2;
        let mut sizesmall = [MAGICINTS[smallidx as usize] as u32; 3];
        let larger = MAGICINTS[maxidx as usize] / 2;

        let mut encoder = Encoder::new(natoms * 3 * 4 + 64);
        let mut prevcoord = [0i32; 3];
        let mut prevrun = -1i32;
        let mut i = 0usize;
        while i < natoms {
            let mut is_small = 0i32;
            let mut is_smaller;
            if smallidx < maxidx
                && i >= 1
                && (ip[i * 3] - prevcoord[0]).abs() < larger
                && (ip[i * 3 + 1] - prevcoord[1]).abs() < larger
                && (ip[i * 3 + 2] - prevcoord[2]).abs() < larger
            {
                is_smaller = 1;
            } else if smallidx > minidx {
                is_smaller = -1;
            } else {
                is_smaller = 0;
            }
            if i + 1 < natoms
                && (ip[i * 3] - ip[i * 3 + 3]).abs() < smallnum
                && (ip[i * 3 + 1] - ip[i * 3 + 4]).abs() < smallnum
                && (ip[i * 3 + 2] - ip[i * 3 + 5]).abs() < smallnum
            {
                ip.swap(i * 3, i * 3 + 3);
                ip.swap(i * 3 + 1, i * 3 + 4);
                ip.swap(i * 3 + 2, i * 3 + 5);
                is_small = 1;
            }
            if bitsize == 0 {
                encoder.sendbits(bitsizeint[0], ip[i * 3] - minint[0]);
                encoder.sendbits(bitsizeint[1], ip[i * 3 + 1] - minint[1]);
                encoder.sendbits(bitsizeint[2], ip[i * 3 + 2] - minint[2]);
            } else {
                encoder.sendints(
                    bitsize,
                    &sizeint,
                    &[
                        (ip[i * 3] - minint[0]) as u32,
                        (ip[i * 3 + 1] - minint[1]) as u32,
                        (ip[i * 3 + 2] - minint[2]) as u32,
                    ],
                );
            }
            prevcoord = [ip[i * 3], ip[i * 3 + 1], ip[i * 3 + 2]];
            i += 1;

            let mut run = 0i32;
            if is_small == 0 && is_smaller == -1 {
                is_smaller = 0;
            }
            let mut tmpcoord = [0i32; 30];
            while is_small != 0 && run < 8 * 3 {
                if is_smaller == -1 {
                    let d0 = ip[i * 3] - prevcoord[0];
                    let d1 = ip[i * 3 + 1] - prevcoord[1];
                    let d2 = ip[i * 3 + 2] - prevcoord[2];
                    if d0 * d0 + d1 * d1 + d2 * d2 >= smaller * smaller {
                        is_smaller = 0;
                    }
                }
                tmpcoord[run as usize] = ip[i * 3] - prevcoord[0] + smallnum;
                run += 1;
                tmpcoord[run as usize] = ip[i * 3 + 1] - prevcoord[1] + smallnum;
                run += 1;
                tmpcoord[run as usize] = ip[i * 3 + 2] - prevcoord[2] + smallnum;
                run += 1;
                prevcoord = [ip[i * 3], ip[i * 3 + 1], ip[i * 3 + 2]];
                i += 1;
                is_small = 0;
                if i < natoms
                    && (ip[i * 3] - prevcoord[0]).abs() < smallnum
                    && (ip[i * 3 + 1] - prevcoord[1]).abs() < smallnum
                    && (ip[i * 3 + 2] - prevcoord[2]).abs() < smallnum
                {
                    is_small = 1;
                }
            }
            if run != prevrun || is_smaller != 0 {
                prevrun = run;
                encoder.sendbits(1, 1);
                encoder.sendbits(5, run + is_smaller + 1);
            } else {
                encoder.sendbits(1, 0);
            }
            let mut k = 0i32;
            while k < run {
                let base = k as usize;
                encoder.sendints(
                    smallidx,
                    &sizesmall,
                    &[
                        tmpcoord[base] as u32,
                        tmpcoord[base + 1] as u32,
                        tmpcoord[base + 2] as u32,
                    ],
                );
                k += 3;
            }
            if is_smaller != 0 {
                smallidx += is_smaller;
                if is_smaller < 0 {
                    smallnum = smaller;
                    smaller = MAGICINTS[(smallidx - 1) as usize] / 2;
                } else {
                    smaller = smallnum;
                    smallnum = MAGICINTS[smallidx as usize] / 2;
                }
                sizesmall = [MAGICINTS[smallidx as usize] as u32; 3];
            }
        }
        if encoder.lastbits != 0 {
            encoder.index += 1;
        }

        let nbytes = encoder.index;
        push_i32(out, nbytes as i32);
        out.extend_from_slice(&encoder.data[..nbytes]);
        let pad = (round_up_4(nbytes as i64) - nbytes as i64) as usize;
        out.resize(out.len() + pad, 0);
    }

    #[test]
    fn sizeofint_matches_reference() {
        assert_eq!(sizeofint(0), 0);
        assert_eq!(sizeofint(1), 1);
        assert_eq!(sizeofint(2), 2);
        assert_eq!(sizeofint(255), 8);
        assert_eq!(sizeofint(256), 9);
    }

    #[test]
    fn reads_small_uncompressed_frames() {
        let box9 = [2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0];
        let frame0 = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];
        let frame1 = [1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8];
        let mut bytes = Vec::new();
        encode_frame(&mut bytes, XTC_MAGIC, 3, 0, 1.5, &box9, &frame0, 1000.0);
        encode_frame(&mut bytes, XTC_MAGIC, 3, 1, 3.0, &box9, &frame1, 1000.0);

        let traj = read_xtc_from(Cursor::new(bytes)).unwrap();
        assert_eq!(traj.frame_count(), 2);
        assert_eq!(traj.natoms(), 3);
        // nm -> Angstrom.
        assert!((traj.position(0, 0).x - 1.0).abs() < 1e-4);
        assert!((traj.position(1, 2).z - 18.0).abs() < 1e-4);
        assert!((traj.time(0) - 1.5).abs() < 1e-4);
        assert!((traj.time(1) - 3.0).abs() < 1e-4);
    }

    #[test]
    fn round_trips_compressed_frames() {
        let precision = 1000.0f32;
        // 24 three-atom "molecules": atoms within a molecule sit close together
        // (exercising the run-length / small-index path), molecules are spaced
        // apart (exercising the full-coordinate path between runs).
        let molecules = 24usize;
        let natoms = molecules * 3;
        let box9 = [8.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0, 0.0, 8.0];

        let make_frame = |shift: f32| {
            let mut coords = Vec::with_capacity(natoms * 3);
            for m in 0..molecules {
                let cx = 0.25 * m as f32 + shift;
                let cy = 1.0 + (m as f32 * 0.13).sin();
                let cz = 1.0 + (m as f32 * 0.17).cos();
                for a in 0..3 {
                    coords.push(cx + 0.01 * a as f32);
                    coords.push(cy + 0.012 * a as f32);
                    coords.push(cz - 0.008 * a as f32);
                }
            }
            coords
        };

        let frames: Vec<Vec<f32>> = (0..4).map(|f| make_frame(f as f32 * 0.05)).collect();
        let mut bytes = Vec::new();
        for (f, coords) in frames.iter().enumerate() {
            encode_frame(
                &mut bytes,
                XTC_MAGIC,
                natoms,
                f as i32,
                f as f32 * 2.0,
                &box9,
                coords,
                precision,
            );
        }

        let traj = read_xtc_from(Cursor::new(bytes)).unwrap();
        assert_eq!(traj.frame_count(), frames.len());
        assert_eq!(traj.natoms(), natoms);

        // Quantization error is at most half a step (0.0005 nm = 0.005 A).
        for (f, coords) in frames.iter().enumerate() {
            for a in 0..natoms {
                let decoded = traj.position(f, a);
                assert!(
                    (decoded.x - coords[a * 3] * 10.0).abs() < 0.02,
                    "frame {f} atom {a} x"
                );
                assert!(
                    (decoded.y - coords[a * 3 + 1] * 10.0).abs() < 0.02,
                    "frame {f} atom {a} y"
                );
                assert!(
                    (decoded.z - coords[a * 3 + 2] * 10.0).abs() < 0.02,
                    "frame {f} atom {a} z"
                );
            }
            assert!((traj.time(f) - f as f32 * 2.0).abs() < 1e-4);
        }
    }

    #[test]
    fn empty_stream_yields_empty_trajectory() {
        let traj = read_xtc_from(Cursor::new(Vec::new())).unwrap();
        assert!(traj.is_empty());
        assert_eq!(traj.frame_count(), 0);
    }
}
