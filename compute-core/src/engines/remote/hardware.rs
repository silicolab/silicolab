//! Static hardware inventory of a remote host, gathered over SSH.
//!
//! One aggregate command emits marker-delimited sections (`#CPU`, `#NPROC`,
//! `#MEM`, `#GPU`) so a single round trip captures everything; the parsing is a
//! pure function ([`parse_remote_hardware`]) over that text, kept testable and
//! free of any I/O. Every tool is wrapped so a missing one (e.g. `nvidia-smi` on
//! a GPU-less box) yields an empty section rather than failing the probe.

/// What we could read about a remote machine. Missing fields stay `None`/empty
/// when the corresponding tool isn't present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteHardwareInfo {
    pub cpu_model: String,
    /// Physical cores (sockets × cores-per-socket), when `lscpu` reports both.
    pub cores: Option<usize>,
    /// Logical CPUs / threads (`nproc`, or `lscpu` "CPU(s)").
    pub threads: Option<usize>,
    pub ram_bytes: Option<u64>,
    /// One entry per detected GPU (the raw `nvidia-smi` "name, memory" line).
    pub gpus: Vec<String>,
}

/// The remote script: marker-delimited sections, each tool silenced and the
/// whole thing forced to exit 0 (`; true`) so a missing tool isn't read as a
/// connection failure by [`super::run_probe_command`].
pub const PROBE_SCRIPT: &str = "echo '#CPU'; lscpu 2>/dev/null; \
     echo '#NPROC'; nproc 2>/dev/null; \
     echo '#MEM'; free -b 2>/dev/null; \
     echo '#GPU'; nvidia-smi --query-gpu=name,memory.total --format=csv,noheader 2>/dev/null; \
     true";

/// Parse the marker-delimited output of [`PROBE_SCRIPT`] into a structured
/// inventory. Tolerant: unknown lines are ignored and missing sections leave
/// their fields empty.
pub fn parse_remote_hardware(stdout: &str) -> RemoteHardwareInfo {
    let (cpu, nproc, mem, gpu) = split_sections(stdout);

    let cores = match (
        lscpu_field(&cpu, "Socket(s)").and_then(|s| s.parse::<usize>().ok()),
        lscpu_field(&cpu, "Core(s) per socket").and_then(|s| s.parse::<usize>().ok()),
    ) {
        (Some(sockets), Some(per_socket)) => Some(sockets * per_socket),
        _ => None,
    };

    RemoteHardwareInfo {
        cpu_model: lscpu_field(&cpu, "Model name")
            .unwrap_or("Unknown CPU")
            .to_string(),
        cores,
        threads: parse_first_usize(&nproc)
            .or_else(|| lscpu_field(&cpu, "CPU(s)").and_then(|s| s.parse().ok())),
        ram_bytes: parse_free_total(&mem),
        gpus: gpu
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

/// Carve the output into its four sections by marker line.
fn split_sections(stdout: &str) -> (String, String, String, String) {
    let (mut cpu, mut nproc, mut mem, mut gpu) =
        (String::new(), String::new(), String::new(), String::new());
    let mut current: Option<&mut String> = None;
    for line in stdout.lines() {
        match line.trim() {
            "#CPU" => current = Some(&mut cpu),
            "#NPROC" => current = Some(&mut nproc),
            "#MEM" => current = Some(&mut mem),
            "#GPU" => current = Some(&mut gpu),
            _ => {
                if let Some(buf) = current.as_deref_mut() {
                    buf.push_str(line);
                    buf.push('\n');
                }
            }
        }
    }
    (cpu, nproc, mem, gpu)
}

/// Value of an `lscpu` `Key: value` line, trimmed (e.g. `"Model name"`).
fn lscpu_field<'a>(lscpu: &'a str, key: &str) -> Option<&'a str> {
    lscpu.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

/// First parseable unsigned integer across the lines (for `nproc`).
fn parse_first_usize(text: &str) -> Option<usize> {
    text.lines().find_map(|l| l.trim().parse().ok())
}

/// Total bytes from the `Mem:` row of `free -b` (its first numeric column).
fn parse_free_total(free: &str) -> Option<u64> {
    free.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Mem:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

/// Dynamic per-GPU stats for the live remote monitor. `nounits` → bare numbers;
/// `[N/A]` → `None`. The trailing `; true` makes a missing `nvidia-smi` an empty
/// result rather than a non-zero exit (which `run_probe_command` treats as a
/// transport error).
pub const GPU_STATS_SCRIPT: &str = "nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,\
     temperature.gpu,power.draw --format=csv,noheader,nounits 2>/dev/null; true";

/// One remote GPU's live reading (parsed from one `GPU_STATS_SCRIPT` row).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteGpuStat {
    pub index: u32,
    pub name: String,
    pub util_pct: Option<f32>,
    pub vram_used_mib: Option<u64>,
    pub vram_total_mib: Option<u64>,
    pub temp_c: Option<u32>,
    pub power_w: Option<f32>,
}

/// Parse the CSV output of [`GPU_STATS_SCRIPT`] (one GPU per line). Tolerant: a row
/// without a parseable leading index is skipped (banner/warning lines); each field
/// is parsed independently with `[N/A]`/empty → `None`.
pub fn parse_remote_gpu_stats(stdout: &str) -> Vec<RemoteGpuStat> {
    stdout
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            if fields.len() < 7 {
                return None;
            }
            let index = fields[0].parse::<u32>().ok()?;
            Some(RemoteGpuStat {
                index,
                name: fields[1].to_string(),
                util_pct: parse_gpu_field(fields[2]),
                vram_used_mib: parse_gpu_field(fields[3]),
                vram_total_mib: parse_gpu_field(fields[4]),
                temp_c: parse_gpu_field(fields[5]),
                power_w: parse_gpu_field(fields[6]),
            })
        })
        .collect()
}

/// Parse one `nounits` field; `[N/A]`/`N/A`/empty → `None`.
fn parse_gpu_field<T: std::str::FromStr>(field: &str) -> Option<T> {
    let f = field.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("[n/a]") || f.eq_ignore_ascii_case("n/a") {
        return None;
    }
    f.parse::<T>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#CPU
Architecture:                       x86_64
CPU(s):                             32
Socket(s):                          1
Core(s) per socket:                 16
Thread(s) per core:                 2
Model name:                         AMD Ryzen 9 7950X 16-Core Processor
#NPROC
32
#MEM
               total        used        free      shared  buff/cache   available
Mem:     67234567890 12000000000 40000000000   123456789 15000000000 54000000000
Swap:           0           0           0
#GPU
NVIDIA GeForce RTX 4090, 24564 MiB
";

    #[test]
    fn parses_full_inventory() {
        let info = parse_remote_hardware(SAMPLE);
        assert_eq!(info.cpu_model, "AMD Ryzen 9 7950X 16-Core Processor");
        assert_eq!(info.cores, Some(16)); // 1 socket × 16 cores
        assert_eq!(info.threads, Some(32)); // nproc
        assert_eq!(info.ram_bytes, Some(67_234_567_890));
        assert_eq!(info.gpus, vec!["NVIDIA GeForce RTX 4090, 24564 MiB"]);
    }

    #[test]
    fn tolerates_missing_gpu_and_nproc() {
        // No #NPROC section, empty #GPU: threads fall back to lscpu "CPU(s)".
        let text = "#CPU
CPU(s):                             8
Socket(s):                          1
Core(s) per socket:                 4
Model name:                         Intel(R) Core(TM) i7
#MEM
               total        used        free
Mem:     16000000000  1000000000 15000000000
#GPU
";
        let info = parse_remote_hardware(text);
        assert_eq!(info.cpu_model, "Intel(R) Core(TM) i7");
        assert_eq!(info.cores, Some(4));
        assert_eq!(info.threads, Some(8)); // lscpu CPU(s) fallback
        assert_eq!(info.ram_bytes, Some(16_000_000_000));
        assert!(info.gpus.is_empty());
    }

    #[test]
    fn empty_output_yields_unknown_cpu() {
        let info = parse_remote_hardware("");
        assert_eq!(info.cpu_model, "Unknown CPU");
        assert_eq!(info.cores, None);
        assert_eq!(info.threads, None);
        assert_eq!(info.ram_bytes, None);
        assert!(info.gpus.is_empty());
    }

    #[test]
    fn parses_single_gpu_stats() {
        let out = "0, NVIDIA GeForce RTX 0000, 23, 436, 8192, 50, 17.66\n";
        let gpus = parse_remote_gpu_stats(out);
        assert_eq!(gpus.len(), 1);
        let g = &gpus[0];
        assert_eq!(g.index, 0);
        assert_eq!(g.name, "NVIDIA GeForce RTX 0000");
        assert_eq!(g.util_pct, Some(23.0));
        assert_eq!(g.vram_used_mib, Some(436));
        assert_eq!(g.vram_total_mib, Some(8192));
        assert_eq!(g.temp_c, Some(50));
        assert_eq!(g.power_w, Some(17.66));
    }

    #[test]
    fn parses_multi_gpu_and_na_fields() {
        let out = "0, GPU A, 10, 100, 8192, 40, 15.0\n\
                   1, GPU B, [N/A], [N/A], 16384, [N/A], [N/A]\n";
        let gpus = parse_remote_gpu_stats(out);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[1].index, 1);
        assert_eq!(gpus[1].name, "GPU B");
        assert_eq!(gpus[1].util_pct, None);
        assert_eq!(gpus[1].vram_used_mib, None);
        assert_eq!(gpus[1].vram_total_mib, Some(16384));
        assert_eq!(gpus[1].temp_c, None);
        assert_eq!(gpus[1].power_w, None);
    }

    #[test]
    fn empty_or_no_nvidia_smi_yields_no_gpus() {
        assert!(parse_remote_gpu_stats("").is_empty());
        assert!(parse_remote_gpu_stats("\n  \n").is_empty());
    }

    #[test]
    fn skips_rows_with_unparseable_index() {
        let out = "some warning line\n0, GPU A, 5, 100, 8192, 30, 10.0\n";
        let gpus = parse_remote_gpu_stats(out);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].index, 0);
    }
}
