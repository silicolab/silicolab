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
